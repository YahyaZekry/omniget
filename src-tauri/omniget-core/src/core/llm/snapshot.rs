//! Undo by shadow git: one bare-ish git dir per workspace under the app data
//! dir, never inside the user's repo. Before the first writing tool of a turn
//! the whole work tree is staged into the shadow index and `write-tree` gives a
//! tree id; undo stages the present and `read-tree --reset -u` back to that
//! tree. The user's own `.git` is only borrowed through `alternates`, so no
//! object is copied twice and nothing of theirs is touched. Shape read from
//! opencode (`snapshot/index.ts`) and cline checkpoints.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ERR_SNAPSHOT: &str = "ERR_CODE_SNAPSHOT";
pub const ERR_NOTHING_TO_UNDO: &str = "ERR_CODE_NOTHING_TO_UNDO";

static ROOT: RwLock<Option<PathBuf>> = RwLock::new(None);

pub fn set_root(path: PathBuf) {
    *ROOT.write().unwrap_or_else(|e| e.into_inner()) = Some(path);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub request: String,
    pub tree: String,
    pub at_ms: u64,
    /// Messages the conversation held when this turn began, so undo can take
    /// the turn's bubbles back too. `None` in entries written before 0.10.0.
    #[serde(default)]
    pub messages_before: Option<usize>,
}

/// Turn marks of this process: request id → messages the conversation held
/// when the turn began. The Coordinator sets one per turn; `before_write`
/// copies it into the entry it pushes.
static MARKS: RwLock<Option<std::collections::HashMap<String, usize>>> = RwLock::new(None);

/// Why the last snapshot of a workspace failed (no `git`, a read-only disk).
/// The chip shows it: an undo that cannot happen must not look available.
static FAILURES: RwLock<Option<std::collections::HashMap<PathBuf, String>>> = RwLock::new(None);

pub fn mark_turn(request: &str, messages_before: usize) {
    let mut marks = MARKS.write().unwrap_or_else(|e| e.into_inner());
    let map = marks.get_or_insert_with(Default::default);
    if map.len() > 256 {
        map.clear();
    }
    map.insert(request.to_string(), messages_before);
}

fn mark_of(request: &str) -> Option<usize> {
    MARKS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(request).copied())
}

fn note_failure(workspace: &Path, error: Option<&str>) {
    let mut all = FAILURES.write().unwrap_or_else(|e| e.into_inner());
    let map = all.get_or_insert_with(Default::default);
    match error {
        Some(e) => {
            map.insert(workspace.to_path_buf(), e.to_string());
        }
        None => {
            map.remove(workspace);
        }
    }
}

/// `git` resolvable on the PATH. Checked without spawning anything.
pub fn git_available() -> bool {
    let exe = if cfg!(windows) { "git.exe" } else { "git" };
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
        .unwrap_or(false)
}

/// What stands between this workspace and an undo, if anything.
pub fn problem(workspace: &Path) -> Option<String> {
    if !git_available() {
        return Some(format!(
            "{ERR_SNAPSHOT}: git was not found on the PATH, so turns cannot be undone"
        ));
    }
    FAILURES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(workspace).cloned())
}

/// What `undo` put back.
#[derive(Debug, Clone, Serialize)]
pub struct Undone {
    pub files: Vec<String>,
    pub messages_before: Option<usize>,
}

fn git_dir(workspace: &Path) -> Result<PathBuf, String> {
    let root = ROOT
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or_else(|| format!("{ERR_SNAPSHOT}: no snapshot dir"))?;
    let mut hasher = Sha256::new();
    hasher.update(workspace.to_string_lossy().as_bytes());
    let hash: String = hasher
        .finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(root.join(hash))
}

async fn git(dir: &Path, workspace: &Path, args: &[&str]) -> Result<String, String> {
    let out = tokio::process::Command::new("git")
        .arg("--git-dir")
        .arg(dir)
        .arg("--work-tree")
        .arg(workspace)
        .args(["-c", "core.autocrlf=false", "-c", "core.quotepath=false"])
        .args(args)
        .current_dir(workspace)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("{ERR_SNAPSHOT}: git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{ERR_SNAPSHOT}: git {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn ensure(workspace: &Path) -> Result<PathBuf, String> {
    let dir = git_dir(workspace)?;
    if !dir.join("HEAD").exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{ERR_SNAPSHOT}: {e}"))?;
        git(&dir, workspace, &["init", "-q"]).await?;
        let real_objects = workspace.join(".git").join("objects");
        if real_objects.is_dir() {
            let info = dir.join("objects").join("info");
            let _ = std::fs::create_dir_all(&info);
            let _ = std::fs::write(
                info.join("alternates"),
                format!("{}\n", real_objects.display()),
            );
        }
        let info = dir.join("info");
        let _ = std::fs::create_dir_all(&info);
        let _ = std::fs::write(
            info.join("exclude"),
            "node_modules/\ntarget/\ndist/\nbuild/\n.svelte-kit/\n*.omniget-tmp\n",
        );
    }
    Ok(dir)
}

async fn stage_tree(dir: &Path, workspace: &Path) -> Result<String, String> {
    git(dir, workspace, &["add", "-A", "."]).await?;
    git(dir, workspace, &["write-tree"]).await
}

fn stack_file(dir: &Path, conversation: &str) -> PathBuf {
    let safe: String = conversation
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("omniget-undo-{safe}.json"))
}

fn load(dir: &Path, conversation: &str) -> Vec<Entry> {
    std::fs::read(stack_file(dir, conversation))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn store(dir: &Path, conversation: &str, stack: &[Entry]) {
    if let Ok(bytes) = serde_json::to_vec(stack) {
        let _ = std::fs::write(stack_file(dir, conversation), bytes);
    }
}

/// Called before a writing tool runs. A failure is remembered for the UI.
pub async fn before_write(
    workspace: &Path,
    conversation: &str,
    request: &str,
) -> Result<(), String> {
    let out = before_write_inner(workspace, conversation, request).await;
    note_failure(workspace, out.as_ref().err().map(String::as_str));
    out
}

/// The snapshot itself. One snapshot per turn: a second write of
/// the same `request` is a no-op.
async fn before_write_inner(
    workspace: &Path,
    conversation: &str,
    request: &str,
) -> Result<(), String> {
    let dir = ensure(workspace).await?;
    let mut stack = load(&dir, conversation);
    if stack.last().map(|e| e.request == request).unwrap_or(false) {
        return Ok(());
    }
    let tree = stage_tree(&dir, workspace).await?;
    stack.push(Entry {
        request: request.to_string(),
        messages_before: mark_of(request),
        tree,
        at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    });
    if stack.len() > 50 {
        let cut = stack.len() - 50;
        stack.drain(..cut);
    }
    store(&dir, conversation, &stack);
    Ok(())
}

pub fn depth(workspace: &Path, conversation: &str) -> usize {
    git_dir(workspace)
        .map(|d| load(&d, conversation).len())
        .unwrap_or(0)
}

/// Put the work tree back to how it was before the last writing turn. Returns
/// the files that changed back.
pub async fn undo(workspace: &Path, conversation: &str) -> Result<Undone, String> {
    let dir = ensure(workspace).await?;
    let mut stack = load(&dir, conversation);
    let entry = stack.pop().ok_or_else(|| {
        format!("{ERR_NOTHING_TO_UNDO}: no turn of this conversation wrote files")
    })?;
    let now = stage_tree(&dir, workspace).await?;
    let changed = git(&dir, workspace, &["diff", "--name-only", &entry.tree, &now]).await?;
    git(
        &dir,
        workspace,
        &["read-tree", "--reset", "-u", &entry.tree],
    )
    .await?;
    store(&dir, conversation, &stack);
    Ok(Undone {
        files: changed
            .lines()
            .map(str::to_string)
            .filter(|l| !l.is_empty())
            .collect(),
        messages_before: entry.messages_before,
    })
}
