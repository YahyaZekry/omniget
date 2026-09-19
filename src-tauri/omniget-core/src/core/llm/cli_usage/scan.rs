//! Incremental scanner over the CLI history folders.
//!
//! Rules that the budget and the privacy promise rest on:
//!
//! * **Only `*.jsonl` is ever opened.** `.credentials.json`, `*.lock`,
//!   `*.sock` and everything else are filtered out by extension before any
//!   `File::open`, so the scanner physically cannot read a credential file.
//! * **Resume by offset.** `ScanState` remembers how many bytes of each file
//!   were already consumed. Only whole lines advance the offset, so a line the
//!   CLI is still writing is ignored now and read on the next pass.
//! * **Rewrite detection.** A file shorter than its recorded offset was
//!   rotated or rewritten, so it is read from zero again.
//! * Files are read on a small thread pool (one chunk of the file list per
//!   thread), which is what keeps a cold 500 MB history inside the budget.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::parse::{codex_tool_name, CliKind, CliUsageEntry, FileParser, Parsed, RateLimitSample};

/// How much of a history file is read per syscall.
const CHUNK: usize = 4 * 1024 * 1024;

/// Stable error codes the UI maps (house style: `ERR_*`).
pub const ERR_CLI_USAGE_IO: &str = "ERR_CLI_USAGE_IO";
pub const ERR_CLI_USAGE_STATE: &str = "ERR_CLI_USAGE_STATE";

/// One folder to sweep, with the account it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRoot {
    pub cli: CliKind,
    /// Account label; `"default"` for the user's own config dir.
    pub account: String,
    pub path: PathBuf,
}

impl ScanRoot {
    pub fn new(cli: CliKind, account: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            cli,
            account: account.into(),
            path: path.into(),
        }
    }

    /// A bare path, with the CLI guessed from the folder name. Used by the
    /// `scan(&[PathBuf], ..)` entry point of the contract.
    pub fn guess(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let looks_codex = path
            .components()
            .any(|c| matches!(c.as_os_str().to_str(), Some(".codex") | Some("codex")));
        Self::new(
            if looks_codex {
                CliKind::Codex
            } else {
                CliKind::Claude
            },
            "default",
            path,
        )
    }
}

/// Byte offset already consumed per file. Persisted as
/// `<app_data>/llm/cli-usage-state.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanState {
    offsets: HashMap<PathBuf, u64>,
}

impl ScanState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn offset(&self, path: &Path) -> u64 {
        self.offsets.get(path).copied().unwrap_or(0)
    }

    pub fn set_offset(&mut self, path: impl Into<PathBuf>, offset: u64) {
        self.offsets.insert(path.into(), offset);
    }

    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// Forget everything, so the next scan re-reads the whole history.
    pub fn clear(&mut self) {
        self.offsets.clear();
    }

    /// Drop entries whose file no longer exists, so the state file does not
    /// grow forever with deleted sessions.
    pub fn prune_missing(&mut self) {
        self.offsets.retain(|p, _| p.exists());
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Atomic write (tmp + rename), same shape as `roster_store`.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{ERR_CLI_USAGE_STATE}: {e}"))?;
        }
        let text =
            serde_json::to_string(self).map_err(|e| format!("{ERR_CLI_USAGE_STATE}: {e}"))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{ERR_CLI_USAGE_STATE}: {e}"))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{ERR_CLI_USAGE_STATE}: {e}"))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ScanStats {
    pub files_seen: u32,
    pub files_read: u32,
    pub bytes_read: u64,
    pub lines: u64,
    /// Lines left for the next pass because they had no terminating newline.
    pub partial_lines: u32,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ScanOutcome {
    pub entries: Vec<CliUsageEntry>,
    /// Rate-limit samples found in the history itself (Codex writes them).
    pub limits: Vec<(String, RateLimitSample)>,
    pub stats: ScanStats,
}

/// Contract entry point: sweep bare folders, guessing the CLI from the path.
pub fn scan(roots: &[PathBuf], state: &mut ScanState) -> Vec<CliUsageEntry> {
    let roots: Vec<ScanRoot> = roots.iter().map(ScanRoot::guess).collect();
    scan_roots(&roots, state).entries
}

/// Full sweep: entries, rate-limit samples and statistics.
pub fn scan_roots(roots: &[ScanRoot], state: &mut ScanState) -> ScanOutcome {
    let started = std::time::Instant::now();
    let mut jobs: Vec<Job> = Vec::new();
    for root in roots {
        for (path, project) in walk_jsonl(&root.path) {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let mut from = state.offset(&path);
            if from > size {
                from = 0; // rotated or rewritten
            }
            jobs.push(Job {
                cli: root.cli,
                account: root.account.clone(),
                project,
                path,
                from,
                size,
            });
        }
    }
    let files_seen = jobs.len() as u32;
    let todo: Vec<Job> = jobs.into_iter().filter(|j| j.size > j.from).collect();
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8)
        .min(todo.len().max(1));
    let chunk = todo.len().div_ceil(threads.max(1)).max(1);
    let mut results: Vec<FileResult> = Vec::with_capacity(todo.len());
    std::thread::scope(|s| {
        let mut handles = Vec::new();
        for slice in todo.chunks(chunk) {
            handles.push(s.spawn(move || slice.iter().map(read_file).collect::<Vec<_>>()));
        }
        for h in handles {
            if let Ok(part) = h.join() {
                results.extend(part);
            }
        }
    });

    let mut out = ScanOutcome::default();
    out.stats.files_seen = files_seen;
    for r in results {
        out.stats.files_read += 1;
        out.stats.bytes_read += r.consumed.saturating_sub(r.from);
        out.stats.lines += r.lines;
        out.stats.partial_lines += u32::from(r.partial);
        state.set_offset(r.path, r.consumed);
        out.entries.extend(r.entries);
        out.limits.extend(r.limits);
    }
    out.entries.sort_by_key(|e| e.ts_ms);
    out.limits.sort_by_key(|(_, l)| l.observed_at_ms);
    out.stats.elapsed_ms = started.elapsed().as_millis() as u64;
    out
}

#[derive(Debug, Clone)]
struct Job {
    cli: CliKind,
    account: String,
    project: String,
    path: PathBuf,
    from: u64,
    size: u64,
}

struct FileResult {
    path: PathBuf,
    from: u64,
    consumed: u64,
    lines: u64,
    partial: bool,
    entries: Vec<CliUsageEntry>,
    limits: Vec<(String, RateLimitSample)>,
}

fn read_file(job: &Job) -> FileResult {
    let mut res = FileResult {
        path: job.path.clone(),
        from: job.from,
        consumed: job.from,
        lines: 0,
        partial: false,
        entries: Vec::new(),
        limits: Vec::new(),
    };
    let session_hint = job
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let Ok(file) = std::fs::File::open(&job.path) else {
        return res;
    };
    let mut file = file;
    if job.from > 0 && file.seek(SeekFrom::Start(job.from)).is_err() {
        return res;
    }
    let mut parser = FileParser::new(job.cli, &job.account, &job.project, session_hint);
    // Whole blocks are read at once and split on newlines in memory: one
    // `read` syscall per 4 MiB instead of one `read_until` per line.
    let mut block = vec![0u8; CHUNK];
    let mut carry: Vec<u8> = Vec::with_capacity(CHUNK + 64 * 1024);
    loop {
        let n = match file.read(&mut block) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        carry.extend_from_slice(&block[..n]);
        let mut start = 0usize;
        while let Some(pos) = carry[start..].iter().position(|&b| b == b'\n') {
            let line = &carry[start..start + pos];
            res.consumed += (pos + 1) as u64;
            res.lines += 1;
            handle_line(line, job, &mut parser, &mut res);
            start += pos + 1;
        }
        carry.drain(..start);
    }
    // Anything left has no newline: the CLI is still writing it, so it stays
    // for the next pass and the offset does not move past it.
    res.partial = !carry.is_empty();
    res
}

/// One complete line, without its newline.
fn handle_line(line: &[u8], job: &Job, parser: &mut FileParser, res: &mut FileResult) {
    if !super::parse::may_carry_usage(job.cli, line) {
        if job.cli == CliKind::Codex && super::parse::may_be_tool_call(line) {
            if let Ok(text) = std::str::from_utf8(line) {
                if let Some(tool) = codex_tool_name(text) {
                    // Tool calls sit on their own line in a Codex rollout; the
                    // heat map only needs the name and the hour.
                    res.entries.push(CliUsageEntry {
                        cli: CliKind::Codex,
                        account: job.account.clone(),
                        project: job.project.clone(),
                        ts_ms: super::parse::ts_ms(
                            serde_json::from_str::<serde_json::Value>(text)
                                .ok()
                                .as_ref()
                                .and_then(|v| v.get("timestamp")),
                        )
                        .unwrap_or(0),
                        tools: vec![tool],
                        ..Default::default()
                    });
                }
            }
        }
        return;
    }
    let Ok(text) = std::str::from_utf8(line) else {
        return;
    };
    match parser.feed(text) {
        Parsed::Usage(e) => res.entries.push(*e),
        Parsed::Limits(samples) => {
            for s in samples {
                res.limits.push((job.account.clone(), s));
            }
        }
        Parsed::Ignored => {}
    }
}

/// Every `*.jsonl` below `root`, with the first path component under the root
/// as the project label. Symlinks are not followed and no other extension is
/// ever opened.
pub fn walk_jsonl(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, project)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if kind.is_dir() {
                let child = if project.is_empty() {
                    name
                } else {
                    project.clone()
                };
                stack.push((path, child));
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let label = if project.is_empty() {
                    name.trim_end_matches(".jsonl").to_string()
                } else {
                    project.clone()
                };
                out.push((path, label));
            }
        }
    }
    out.sort();
    out
}

/// Default folders: the user's own Claude and Codex histories plus one folder
/// per isolated account config dir. `CLAUDE_CONFIG_DIR` may hold several paths
/// separated by the platform list separator, as the CLI documents.
pub fn default_roots(accounts: &[(String, PathBuf)]) -> Vec<ScanRoot> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        let claude = home.join(".claude").join("projects");
        if claude.is_dir() {
            roots.push(ScanRoot::new(CliKind::Claude, "default", claude));
        }
        let codex = home.join(".codex").join("sessions");
        if codex.is_dir() {
            roots.push(ScanRoot::new(CliKind::Codex, "default", codex));
        }
    }
    for (account, dir) in accounts {
        let claude = dir.join("projects");
        if claude.is_dir() {
            roots.push(ScanRoot::new(CliKind::Claude, account.clone(), claude));
        }
        let codex = dir.join("sessions");
        if codex.is_dir() {
            roots.push(ScanRoot::new(CliKind::Codex, account.clone(), codex));
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "omniget-cli-usage-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn line(ts: &str, id: &str, out: u64) -> String {
        format!(
            r#"{{"type":"assistant","sessionId":"s1","timestamp":"{ts}","requestId":"{id}","message":{{"id":"{id}","role":"assistant","model":"claude-opus-5","content":[{{"type":"tool_use","name":"Bash","input":{{}}}}],"usage":{{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":{out}}}}}}}"#
        )
    }

    #[test]
    fn a_second_scan_reads_nothing_new() {
        let dir = tmpdir("incremental");
        let f = dir.join("a.jsonl");
        std::fs::write(
            &f,
            format!("{}\n", line("2026-03-04T12:00:00.000Z", "r1", 5)),
        )
        .unwrap();
        let mut state = ScanState::new();
        let first = scan(std::slice::from_ref(&dir), &mut state);
        assert_eq!(first.len(), 1);
        let second = scan(std::slice::from_ref(&dir), &mut state);
        assert!(second.is_empty(), "warm scan must read nothing");

        let mut fh = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        writeln!(fh, "{}", line("2026-03-04T12:05:00.000Z", "r2", 7)).unwrap();
        drop(fh);
        let third = scan(std::slice::from_ref(&dir), &mut state);
        assert_eq!(third.len(), 1);
        assert_eq!(third[0].output_tokens, 7);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_truncated_last_line_is_ignored_then_resumed() {
        let dir = tmpdir("truncated");
        let f = dir.join("a.jsonl");
        let whole = line("2026-03-04T12:00:00.000Z", "r1", 5);
        let next = line("2026-03-04T12:01:00.000Z", "r2", 9);
        let cut = &next[..40];
        std::fs::write(&f, format!("{whole}\n{cut}")).unwrap();
        let mut state = ScanState::new();
        let first = scan(std::slice::from_ref(&dir), &mut state);
        assert_eq!(first.len(), 1, "the half-written line must not count");
        // The CLI finishes the line.
        std::fs::write(&f, format!("{whole}\n{next}\n")).unwrap();
        let second = scan(std::slice::from_ref(&dir), &mut state);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].output_tokens, 9);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_rewritten_shorter_file_is_read_from_zero() {
        let dir = tmpdir("rewrite");
        let f = dir.join("a.jsonl");
        std::fs::write(
            &f,
            format!(
                "{}\n{}\n",
                line("2026-03-04T12:00:00.000Z", "r1", 5),
                line("2026-03-04T12:01:00.000Z", "r2", 6)
            ),
        )
        .unwrap();
        let mut state = ScanState::new();
        assert_eq!(scan(std::slice::from_ref(&dir), &mut state).len(), 2);
        std::fs::write(
            &f,
            format!("{}\n", line("2026-03-04T13:00:00.000Z", "r3", 8)),
        )
        .unwrap();
        let again = scan(std::slice::from_ref(&dir), &mut state);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].output_tokens, 8);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The promise of the section: a credential file is never opened, because
    /// only `*.jsonl` reaches `File::open`.
    #[test]
    fn only_jsonl_files_are_ever_opened() {
        let dir = tmpdir("creds");
        std::fs::write(
            dir.join(".credentials.json"),
            "{\"accessToken\":\"secret\"}",
        )
        .unwrap();
        std::fs::write(dir.join("settings.json"), "{}").unwrap();
        std::fs::write(dir.join("a.jsonl.lock"), "x").unwrap();
        std::fs::write(
            dir.join("a.jsonl"),
            format!("{}\n", line("2026-03-04T12:00:00.000Z", "r1", 5)),
        )
        .unwrap();
        let files = walk_jsonl(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0.file_name().unwrap(), "a.jsonl");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_walk_is_recursive_and_labels_the_project() {
        let dir = tmpdir("walk");
        let proj = dir.join("-home-lorem-ipsum").join("subagents");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("agent-1.jsonl"), "").unwrap();
        std::fs::write(dir.join("-home-lorem-ipsum").join("s1.jsonl"), "").unwrap();
        let files = walk_jsonl(&dir);
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|(_, p)| p == "-home-lorem-ipsum"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn state_survives_a_save_and_load_round_trip() {
        let dir = tmpdir("state");
        let f = dir.join("cli-usage-state.json");
        let mut state = ScanState::new();
        state.set_offset(dir.join("a.jsonl"), 4242);
        state.save(&f).unwrap();
        let back = ScanState::load(&f);
        assert_eq!(back.offset(&dir.join("a.jsonl")), 4242);
        assert_eq!(back.len(), 1);
        let mut back = back;
        back.prune_missing();
        assert!(back.is_empty(), "the file does not exist any more");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_cli_is_guessed_from_the_folder() {
        assert_eq!(
            ScanRoot::guess(PathBuf::from("/home/lorem/.codex/sessions")).cli,
            CliKind::Codex
        );
        assert_eq!(
            ScanRoot::guess(PathBuf::from("/home/lorem/.claude/projects")).cli,
            CliKind::Claude
        );
    }

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cli_usage_fixtures")
    }

    #[test]
    fn the_claude_fixture_parses_the_way_the_real_file_does() {
        let dir = tmpdir("fixture-claude");
        std::fs::copy(
            fixtures().join("claude-2.1.275.jsonl"),
            dir.join("claude-2.1.275.jsonl"),
        )
        .unwrap();
        let mut state = ScanState::new();
        let out = scan_roots(
            &[ScanRoot::new(CliKind::Claude, "default", &dir)],
            &mut state,
        );
        // 3 real turns + the duplicate; the API error, the synthetic model and
        // the half-written last line are out.
        assert_eq!(out.entries.len(), 4, "{:?}", out.entries);
        assert_eq!(out.stats.partial_lines, 1);
        assert!(out.entries.iter().any(|e| e.sidechain));
        assert!(out
            .entries
            .iter()
            .any(|e| e.tools.iter().any(|t| t == "Grep")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_old_claude_fixture_still_parses() {
        let dir = tmpdir("fixture-old");
        std::fs::copy(
            fixtures().join("claude-2.1.226.jsonl"),
            dir.join("claude-2.1.226.jsonl"),
        )
        .unwrap();
        let mut state = ScanState::new();
        let out = scan_roots(
            &[ScanRoot::new(CliKind::Claude, "default", &dir)],
            &mut state,
        );
        assert_eq!(out.entries.len(), 2);
        // Older lines have no `output_tokens_details`: reasoning is 0, not a
        // parse failure.
        assert!(out.entries.iter().all(|e| e.reasoning_tokens == 0));
        assert_eq!(out.entries[0].input_tokens, 12_100);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_codex_fixture_yields_usage_tools_and_windows() {
        let dir = tmpdir("fixture-codex");
        std::fs::copy(
            fixtures().join("codex-rollout.jsonl"),
            dir.join("codex-rollout.jsonl"),
        )
        .unwrap();
        let mut state = ScanState::new();
        let out = scan_roots(&[ScanRoot::new(CliKind::Codex, "work", &dir)], &mut state);
        let billed: Vec<_> = out
            .entries
            .iter()
            .filter(|e| e.input_tokens + e.output_tokens > 0)
            .collect();
        assert_eq!(billed.len(), 2);
        assert_eq!(billed[0].model, "gpt-6-astra");
        assert_eq!(billed[1].model, "gpt-6-mini");
        assert!(out
            .entries
            .iter()
            .any(|e| e.tools.iter().any(|t| t == "exec")));
        assert_eq!(out.limits.len(), 2);
        assert_eq!(out.limits[0].0, "work");
        assert_eq!(out.stats.partial_lines, 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
