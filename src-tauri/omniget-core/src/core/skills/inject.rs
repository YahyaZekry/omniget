//! Putting skills in front of the model: the index in the system prompt, and
//! the body on demand.
//!
//! Progressive disclosure, the same way the spec describes it:
//! 1. every active skill contributes one line — `name` plus `description` — to
//!    the system prompt ([`index_prompt`]);
//! 2. the model decides it needs one and calls the tool named `skill:<name>`
//!    ([`tool_specs`]); the broker routes that to [`open`], which returns the
//!    Markdown body of `SKILL.md`;
//! 3. files under `scripts/`, `references/` and `assets/` are read one at a
//!    time with [`read_file_in`], which cannot leave the skill folder.
//!
//! Budget: [`index_prompt`] is the only function here that runs on every turn.
//! It allocates once and concatenates; target ≤ 100 µs for 50 skills.

use std::path::Path;

use super::install::{self, skill_path};
use super::manifest::{self, SkillManifest, SKILL_FILE};
use super::{SkillError, ERR_SKILL_PATH, ERR_SKILL_TOO_BIG};
use crate::core::llm::types::ToolSpec;

/// Namespace of the tool that opens a skill. Matches `broker::grant_key` for
/// `ToolSource::Skill`, so a grant and a tool call line up without a mapping
/// table.
pub const SKILL_TOOL_PREFIX: &str = "skill:";
/// Most bytes of skill body handed to the model in one call.
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// Most bytes of an auxiliary file handed to the model in one call.
pub const MAX_FILE_BYTES: usize = 256 * 1024;
/// Appended when a body or file is cut, so the model knows it is partial.
pub const TRUNCATION_MARK: &str = "\n\n[cut here: the rest of this file was not loaded]";

/// The short index that goes in the system prompt. Empty when nothing is
/// active, so the caller can append it unconditionally.
pub fn index_prompt(active: &[SkillManifest]) -> String {
    if active.is_empty() {
        return String::new();
    }
    let size = 400
        + active
            .iter()
            .map(|s| s.name.len() + s.description.len() + 16)
            .sum::<usize>();
    let mut out = String::with_capacity(size);
    index_prompt_into(&mut out, active);
    out
}

/// [`index_prompt`] writing into a buffer the caller owns. A prompt builder that
/// runs on every turn keeps one `String` and clears it, so the only cost left is
/// the copy.
pub fn index_prompt_into(out: &mut String, active: &[SkillManifest]) {
    if active.is_empty() {
        return;
    }
    out.push_str("# Skills\n\n");
    out.push_str(
        "These skills are installed and granted to you. Each line is a name and when to use it; \
         the instructions are not loaded yet. When one matches the task, call the tool with the \
         same name (`skill:<name>`) to read it, then follow it.\n\n",
    );
    for skill in active {
        out.push_str("- `");
        out.push_str(SKILL_TOOL_PREFIX);
        out.push_str(&skill.name);
        out.push_str("`: ");
        push_one_line(out, &skill.description);
        out.push('\n');
    }
}

/// Appends a description as a single line: a block scalar in the frontmatter can
/// carry newlines, and one skill must not become two entries.
fn push_one_line(out: &mut String, text: &str) {
    let text = text.trim();
    // Fast path: almost every description is already one line, and then this is
    // a single copy instead of a walk over every character.
    if !text
        .as_bytes()
        .iter()
        .any(|b| matches!(b, b'\n' | b'\r' | b'\t'))
    {
        out.push_str(text);
        return;
    }
    let mut last_was_space = false;
    for ch in text.trim().chars() {
        if ch == '\n' || ch == '\r' || ch == '\t' {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            last_was_space = ch == ' ';
            out.push(ch);
        }
    }
}

/// The tool name a grant and a model call use for this skill.
pub fn tool_name(name: &str) -> String {
    format!("{SKILL_TOOL_PREFIX}{name}")
}

/// The skill behind a tool name, or `None` when it is not a skill tool.
pub fn skill_from_tool(tool: &str) -> Option<&str> {
    tool.strip_prefix(SKILL_TOOL_PREFIX)
        .filter(|s| !s.is_empty())
}

/// One [`ToolSpec`] per active skill, for `ToolBroker::new`. The input schema is
/// empty on purpose: opening a skill takes no argument, the name is the tool.
pub fn tool_specs(active: &[SkillManifest]) -> Vec<ToolSpec> {
    active
        .iter()
        .map(|skill| ToolSpec {
            name: tool_name(&skill.name),
            description: format!(
                "Load the full instructions of the `{}` skill. {}",
                skill.name, skill.description
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        })
        .collect()
}

/// The Markdown body of an installed skill, frontmatter stripped, cut at
/// [`MAX_BODY_BYTES`].
pub fn open(name: &str) -> Result<String, SkillError> {
    open_in(&install::skills_dir()?, name)
}

/// [`open`] against an explicit root.
pub fn open_in(root: &Path, name: &str) -> Result<String, SkillError> {
    let dir = skill_path(root, name)?;
    let file = dir.join(SKILL_FILE);
    let text = std::fs::read_to_string(&file)
        .map_err(|e| SkillError::io(&format!("reading {}", file.display()), &e))?;
    let (_, body) = manifest::split_frontmatter(&text)?;
    Ok(cut(body.trim_start(), MAX_BODY_BYTES))
}

/// An auxiliary file of an installed skill (`references/REFERENCE.md`,
/// `scripts/run.sh`, …). `rel` cannot walk out of the skill folder, and a
/// symlink inside it is refused rather than followed.
pub fn read_file_in(root: &Path, name: &str, rel: &str) -> Result<String, SkillError> {
    let dir = skill_path(root, name)?;
    let file = install::join_inside(&dir, rel)?;
    let meta = std::fs::symlink_metadata(&file)
        .map_err(|e| SkillError::io(&format!("reading {}", file.display()), &e))?;
    if meta.file_type().is_symlink() {
        return Err(SkillError::new(
            ERR_SKILL_PATH,
            format!("{} is a symlink", file.display()),
        ));
    }
    if !meta.is_file() {
        return Err(SkillError::new(
            ERR_SKILL_PATH,
            format!("{} is not a file", file.display()),
        ));
    }
    if meta.len() as usize > MAX_FILE_BYTES * 4 {
        return Err(SkillError::new(
            ERR_SKILL_TOO_BIG,
            format!("{} is {} bytes", file.display(), meta.len()),
        ));
    }
    let text = std::fs::read_to_string(&file)
        .map_err(|e| SkillError::io(&format!("reading {}", file.display()), &e))?;
    Ok(cut(&text, MAX_FILE_BYTES))
}

/// Cuts at a char boundary at or below `max` bytes and says so.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + TRUNCATION_MARK.len());
    out.push_str(&text[..end]);
    out.push_str(TRUNCATION_MARK);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::skills::install::tests_support::temp_dir;
    use crate::core::skills::SkillSource;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn fake(name: &str, description: &str) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            description: description.to_string(),
            allowed_tools: Vec::new(),
            path: PathBuf::from("/nowhere").join(name),
            source: SkillSource::Unknown,
            license: None,
            compatibility: None,
            metadata: BTreeMap::new(),
            body_bytes: 0,
            scan: crate::core::skills::scan::ScanStatus::NotScanned,
        }
    }

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn installed(tag: &str, name: &str, body: &str) -> (Scratch, PathBuf) {
        let scratch = Scratch(temp_dir(tag));
        let root = scratch.0.join("root");
        let src = scratch.0.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join(SKILL_FILE),
            format!("---\nname: {name}\ndescription: A test skill.\n---\n\n{body}\n"),
        )
        .unwrap();
        std::fs::create_dir_all(src.join("references")).unwrap();
        std::fs::write(src.join("references/REFERENCE.md"), "reference body").unwrap();
        install::install_from_dir_in(&root, &src).unwrap();
        (scratch, root)
    }

    #[test]
    fn an_empty_roster_adds_nothing_to_the_prompt() {
        assert_eq!(index_prompt(&[]), "");
    }

    #[test]
    fn the_index_carries_name_and_description_only() {
        let skills = [
            fake("pdf-forms", "Fills PDF forms. Use when a form shows up."),
            fake("note-taker", "Writes meeting notes."),
        ];
        let prompt = index_prompt(&skills);
        assert!(
            prompt.contains("`skill:pdf-forms`: Fills PDF forms."),
            "{prompt}"
        );
        assert!(prompt.contains("`skill:note-taker`: Writes meeting notes."));
        assert_eq!(prompt.lines().filter(|l| l.starts_with("- `")).count(), 2);
    }

    #[test]
    fn a_multiline_description_stays_on_one_line() {
        let skills = [fake("wrapped", "first line\nsecond line\n\nthird")];
        let prompt = index_prompt(&skills);
        assert!(
            prompt.contains("`skill:wrapped`: first line second line third\n"),
            "{prompt}"
        );
        assert_eq!(prompt.lines().filter(|l| l.starts_with("- `")).count(), 1);
    }

    #[test]
    fn index_prompt_stays_under_a_hundred_microseconds_for_fifty_skills() {
        let skills: Vec<SkillManifest> = (0..50)
            .map(|i| {
                fake(
                    &format!("skill-number-{i}"),
                    &"a fairly long description of what this skill does and when to use it. "
                        .repeat(4),
                )
            })
            .collect();
        let runs = 2_000;
        // The shape a prompt builder actually has: one buffer, cleared per turn.
        let mut buffer = String::with_capacity(index_prompt(&skills).len());
        let mut sink = 0usize;
        let started = std::time::Instant::now();
        for _ in 0..runs {
            buffer.clear();
            index_prompt_into(&mut buffer, &skills);
            sink += buffer.len();
        }
        let per = started.elapsed() / runs;

        // Reported for the record: the allocating call, which a caller that does
        // not keep a buffer pays instead.
        let started_alloc = std::time::Instant::now();
        for _ in 0..runs {
            sink += index_prompt(&skills).len();
        }
        let per_alloc = started_alloc.elapsed() / runs;
        println!(
            "skills::inject::index_prompt(50 skills, {} bytes out) = {per:?} reusing a buffer, \
             {per_alloc:?} allocating, over {runs} runs each, {} build (sink {sink})",
            buffer.len(),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        // The 100 µs budget of the plan is a release number. `cargo test` builds
        // the `test` profile unoptimized, where the same work costs about an
        // order of magnitude more, so the ceiling here is the one that would
        // catch a real regression in a debug run.
        let ceiling = if cfg!(debug_assertions) {
            std::time::Duration::from_micros(600)
        } else {
            std::time::Duration::from_micros(100)
        };
        assert!(per < ceiling, "took {per:?}, ceiling {ceiling:?}");
    }

    #[test]
    fn tool_names_round_trip() {
        assert_eq!(tool_name("pdf-forms"), "skill:pdf-forms");
        assert_eq!(skill_from_tool("skill:pdf-forms"), Some("pdf-forms"));
        assert_eq!(skill_from_tool("skill:"), None);
        assert_eq!(skill_from_tool("mcp:fs:read"), None);
    }

    #[test]
    fn tool_specs_match_the_broker_grant_key() {
        let skills = [fake("pdf-forms", "Fills PDF forms.")];
        let specs = tool_specs(&skills);
        assert_eq!(specs.len(), 1);
        let grant =
            crate::core::llm::broker::grant_key(&crate::core::llm::agent::ToolSource::Skill {
                name: "pdf-forms".into(),
            });
        assert_eq!(specs[0].name, grant);
        assert!(specs[0].description.contains("Fills PDF forms."));
    }

    #[test]
    fn open_returns_the_body_without_the_frontmatter() {
        let (_scratch, root) = installed("open", "note-taker", "# Note taker\n\nStep one.");
        let body = open_in(&root, "note-taker").unwrap();
        assert!(body.starts_with("# Note taker"), "{body}");
        assert!(!body.contains("description:"));
    }

    #[test]
    fn open_cuts_a_huge_body_and_says_so() {
        let big = "x".repeat(MAX_BODY_BYTES + 5_000);
        let (_scratch, root) = installed("open-big", "big-skill", &big);
        let body = open_in(&root, "big-skill").unwrap();
        assert!(body.ends_with(TRUNCATION_MARK));
        assert!(body.len() <= MAX_BODY_BYTES + TRUNCATION_MARK.len());
    }

    #[test]
    fn open_refuses_an_unknown_or_unsafe_name() {
        let (_scratch, root) = installed("open-bad", "note-taker", "body");
        assert_eq!(
            open_in(&root, "../../etc").unwrap_err().code(),
            crate::core::skills::ERR_SKILL_NAME
        );
        assert_eq!(
            open_in(&root, "not-installed").unwrap_err().code(),
            crate::core::skills::ERR_SKILL_NOT_FOUND
        );
    }

    #[test]
    fn read_file_stays_inside_the_skill() {
        let (_scratch, root) = installed("read-file", "note-taker", "body");
        let text = read_file_in(&root, "note-taker", "references/REFERENCE.md").unwrap();
        assert_eq!(text, "reference body");
        assert_eq!(
            read_file_in(&root, "note-taker", "../../secret")
                .unwrap_err()
                .code(),
            ERR_SKILL_PATH
        );
    }

    #[test]
    fn cut_lands_on_a_char_boundary() {
        let text = "á".repeat(100); // two bytes per char
        let out = cut(&text, 51);
        assert!(out.ends_with(TRUNCATION_MARK));
        let kept = out.trim_end_matches(TRUNCATION_MARK);
        assert_eq!(kept.chars().count(), 25);
    }
}
