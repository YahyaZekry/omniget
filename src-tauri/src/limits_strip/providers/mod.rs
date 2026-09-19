//! One small reader per assistant. Each file documents where the tool keeps
//! its own credential on each OS and which official endpoint answers.

pub mod claude;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod grok;
pub mod kimi;
pub mod lmstudio;
pub mod ollama;
pub mod opencode;

use super::UsageProvider;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// The ring OmniGet's own roster agents share. It has no reader here: the
/// engine fills it from the telemetry the app already keeps.
pub const OMNIGET_ID: &str = "omniget";

/// Every reader, in the order a fresh install lists them.
pub fn all() -> Vec<Arc<dyn UsageProvider>> {
    vec![
        Arc::new(claude::Claude),
        Arc::new(codex::Codex),
        Arc::new(cursor::Cursor),
        Arc::new(copilot::Copilot),
        Arc::new(grok::Grok),
        Arc::new(kimi::Kimi),
        Arc::new(opencode::OpenCode),
        Arc::new(ollama::Ollama),
        Arc::new(lmstudio::LmStudio),
    ]
}

/// A client for the runtimes on this machine: loopback only, no proxy, and a
/// short deadline because nobody is that far away.
pub fn loopback() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .no_proxy()
        .build()
        .unwrap_or_default()
}

/// What a local runtime reads as while nothing listens on its port.
pub fn not_running() -> super::Reading {
    super::Reading {
        note: Some("not running".into()),
        ..Default::default()
    }
}

/// Offline check for a local runtime: its folder in the home, or its CLI.
pub fn installed(home_dir: &str, cli: &str) -> bool {
    super::home()
        .map(|h| h.join(home_dir).is_dir())
        .unwrap_or(false)
        || which::which(cli).is_ok()
}

/// `$XDG_DATA_HOME` or `~/.local/share`, on every OS: the Node tools that use
/// `xdg-basedir` (OpenCode and friends) resolve it the same way on Windows and
/// macOS, without the platform's native data folder.
pub fn xdg_data_home() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("XDG_DATA_HOME") {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    super::home().map(|h| h.join(".local").join("share"))
}

/// A string that may sit directly at a key or inside an object under one of
/// several field names, as the CLIs' `auth.json` files do.
pub fn string_or_field(v: &serde_json::Value, fields: &[&str]) -> Option<String> {
    if let Some(s) = v.as_str() {
        let s = s.trim();
        return (!s.is_empty()).then(|| s.to_string());
    }
    fields.iter().find_map(|f| {
        v.get(*f)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

/// Runs a helper CLI with no stdin, a deadline, and both streams captured.
pub async fn run_cli(
    program: &std::path::Path,
    args: &[&str],
    envs: &[(&str, &str)],
    deadline: Duration,
) -> Option<(bool, String, String)> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(std::env::temp_dir())
        .kill_on_drop(true);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW: no console flashes on screen every poll.
        cmd.creation_flags(0x0800_0000);
    }
    let child = cmd.spawn().ok()?;
    let out = tokio::time::timeout(deadline, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    Some((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// Opens another tool's SQLite state strictly read-only. `mode=ro` first, so a
/// token the tool just rotated into the WAL is seen; `immutable=1` when the
/// tool has exited and left no `-shm` behind.
pub fn open_sqlite_ro(path: &std::path::Path) -> Option<rusqlite::Connection> {
    use rusqlite::{Connection, OpenFlags};
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let p = path.to_string_lossy().replace('\\', "/");
    let p = if p.starts_with('/') {
        p
    } else {
        format!("/{p}")
    };
    let enc: String = p
        .chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '#' => "%23".to_string(),
            '?' => "%3f".to_string(),
            '%' => "%25".to_string(),
            c => c.to_string(),
        })
        .collect();
    for suffix in ["mode=ro", "immutable=1"] {
        let uri = format!("file:{enc}?{suffix}");
        if let Ok(conn) = Connection::open_with_flags(&uri, flags) {
            let _ = conn.busy_timeout(Duration::from_millis(500));
            let probe = conn
                .prepare("SELECT name FROM sqlite_master LIMIT 1")
                .and_then(|mut s| s.query([]).map(|_| ()));
            if probe.is_ok() {
                return Some(conn);
            }
        }
    }
    None
}

/// `SELECT value FROM ItemTable WHERE key = ?`: the VS Code family's global
/// state table (Cursor).
pub fn vscdb_value(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM ItemTable WHERE key = ?1", [key], |r| {
        r.get::<_, rusqlite::types::Value>(0)
    })
    .ok()
    .and_then(|v| match v {
        rusqlite::types::Value::Text(s) => Some(s),
        rusqlite::types::Value::Blob(b) => String::from_utf8(b).ok(),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ids_are_unique() {
        let mut ids: Vec<_> = all().iter().map(|p| p.id()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);
        assert!(!ids.contains(&OMNIGET_ID));
    }

    #[test]
    fn only_loopback_runtimes_are_local_and_none_polls_too_fast() {
        for p in all() {
            assert_eq!(
                p.local(),
                matches!(p.id(), "ollama" | "lmstudio"),
                "{}",
                p.id()
            );
            assert!(p.poll_interval() >= crate::limits_strip::engine::MIN_INTERVAL);
        }
    }

    #[test]
    fn a_key_is_a_string_or_a_named_field() {
        let v = serde_json::json!({"a": "k1", "b": {"type": "api", "key": " k2 "}, "c": {"x": 1}});
        assert_eq!(string_or_field(&v["a"], &["key"]), Some("k1".into()));
        assert_eq!(
            string_or_field(&v["b"], &["apiKey", "key"]),
            Some("k2".into())
        );
        assert_eq!(string_or_field(&v["c"], &["key"]), None);
    }

    #[test]
    fn another_tools_sqlite_is_opened_read_only() {
        let dir = std::env::temp_dir().join(format!("omniget limits#db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.vscdb");
        {
            let c = rusqlite::Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE ItemTable(key TEXT PRIMARY KEY, value BLOB);
                 INSERT INTO ItemTable VALUES('k','v');",
            )
            .unwrap();
        }
        let conn = open_sqlite_ro(&path).expect("opens");
        assert_eq!(vscdb_value(&conn, "k"), Some("v".into()));
        assert_eq!(vscdb_value(&conn, "missing"), None);
        assert!(conn
            .execute("INSERT INTO ItemTable VALUES('x','y')", [])
            .is_err());
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
