//! Where a provider key is named, and the one-time move of the keys that older
//! builds wrote in plain text.
//!
//! Two files used to hold secrets as text: `tools/ai-keys.json` (the vault, one
//! `key`/`access_token` per entry) and `ai_config.json` (`openai_key`,
//! `anthropic_key`). Both now hold metadata only; the secret lives in
//! [`crate::core::secrets`] under the [`crate::core::secrets::AI_KEYS`]
//! namespace, keyed by the account names below.
//!
//! The migration runs on the first read after an update, silently. It is
//! idempotent: a file with no plaintext left is not rewritten and gets no new
//! backup. The backup it does write is the untouched old file at
//! `<name>.bak`, mode 0600, so a failed move is recoverable and the copy is not
//! world-readable.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

// Declared from `ai_keys.rs` as `ai_keys::migrate`, so `super` is that module.
use super::KeyEntry;
use crate::core::ai::AiConfig;
use crate::core::secrets::{self, AI_KEYS};

// ---------------------------------------------------------------------------
// Account names
// ---------------------------------------------------------------------------

/// The API key of a vault entry. `kind:id`, so the account says which provider
/// it belongs to even when read straight out of the store.
pub fn key_account(kind: &str, id: &str) -> String {
    format!("{}:{}", kind, id)
}

/// A New API panel access token, kept beside the key of the same entry.
pub fn token_account(kind: &str, id: &str) -> String {
    format!("{}:{}#access_token", kind, id)
}

/// The app's own AI config keys, which have no vault entry behind them.
pub const APP_OPENAI_ACCOUNT: &str = "app:openai_key";
pub const APP_ANTHROPIC_ACCOUNT: &str = "app:anthropic_key";

// ---------------------------------------------------------------------------
// Splitting metadata from secrets
// ---------------------------------------------------------------------------

/// Every (account, value) pair an entry owns, secret or not. An empty value is
/// included on purpose: the writer turns it into a delete, so clearing a key in
/// the UI forgets it in the store too.
pub fn accounts_of(entry: &KeyEntry) -> [(String, String); 2] {
    [
        (
            key_account(&entry.kind, &entry.id),
            entry.key.trim().to_string(),
        ),
        (
            token_account(&entry.kind, &entry.id),
            entry.access_token.trim().to_string(),
        ),
    ]
}

/// True when this entry still carries key material that belongs in the store.
pub fn entry_has_plaintext(entry: &KeyEntry) -> bool {
    !entry.key.trim().is_empty() || !entry.access_token.trim().is_empty()
}

/// Blank the secret fields of a list, returning what was taken out. Used by the
/// normal write path and by the migration alike.
pub fn take_secrets(entries: &mut [KeyEntry]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in entries.iter_mut() {
        for (account, value) in accounts_of(entry) {
            out.push((account, value));
        }
        entry.key.clear();
        entry.access_token.clear();
    }
    out
}

/// The same for the app config. Returns the pairs and leaves the config blank.
pub fn take_config_secrets(cfg: &mut AiConfig) -> Vec<(String, String)> {
    let out = vec![
        (
            APP_OPENAI_ACCOUNT.to_string(),
            cfg.openai_key.trim().to_string(),
        ),
        (
            APP_ANTHROPIC_ACCOUNT.to_string(),
            cfg.anthropic_key.trim().to_string(),
        ),
    ];
    cfg.openai_key.clear();
    cfg.anthropic_key.clear();
    out
}

// ---------------------------------------------------------------------------
// Migration
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// How many secrets were moved into the store.
    pub moved: usize,
    /// The backup written, if the file had to be rewritten at all.
    pub backup: Option<PathBuf>,
}

impl Report {
    pub fn ran(&self) -> bool {
        self.backup.is_some()
    }
}

pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(".bak");
    PathBuf::from(name)
}

/// Copy the file aside at 0600 before rewriting it.
fn write_backup(path: &Path) -> Result<PathBuf, String> {
    let target = backup_path(path);
    let bytes = std::fs::read(path)
        .map_err(|e| format!("ai-keys: could not read {}: {}", path.display(), e))?;
    secrets::write_private(&target, &bytes)?;
    Ok(target)
}

/// Move the plaintext out of the vault file. Returns an empty report when the
/// file is missing, unreadable or already migrated.
pub fn migrate_vault_file(path: &Path) -> Report {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Report::default();
    };
    let Ok(mut entries) = serde_json::from_str::<Vec<KeyEntry>>(&text) else {
        return Report::default();
    };
    if !entries.iter().any(entry_has_plaintext) {
        return Report::default();
    }
    let pairs = take_secrets(&mut entries);
    let mut moved = 0usize;
    for (account, value) in pairs {
        if value.is_empty() {
            continue;
        }
        match secrets::set(AI_KEYS, &account, &value) {
            Ok(()) => moved += 1,
            // Rewriting the file after a failed write would lose the key, so
            // stop and leave the file exactly as it was.
            Err(e) => {
                tracing::warn!("[ai-keys] could not move {} into the store: {}", account, e);
                return Report::default();
            }
        }
    }
    let backup = match write_backup(path) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("[ai-keys] backup failed, keeping the file as it is: {}", e);
            return Report::default();
        }
    };
    let Ok(json) = serde_json::to_string_pretty(&entries) else {
        return Report::default();
    };
    if let Err(e) = secrets::write_private(path, json.as_bytes()) {
        tracing::warn!("[ai-keys] could not rewrite the vault: {}", e);
        return Report::default();
    }
    tracing::info!(
        "[ai-keys] moved {} key(s) out of {} into the secret store (backup at {})",
        moved,
        path.display(),
        backup.display()
    );
    Report {
        moved,
        backup: Some(backup),
    }
}

/// The same for `ai_config.json`. The parsed config comes in already read from
/// disk, because the caller needs it anyway; the keys are taken out of it.
pub fn migrate_config_file(path: &Path, cfg: &mut AiConfig) -> Report {
    if cfg.openai_key.trim().is_empty() && cfg.anthropic_key.trim().is_empty() {
        return Report::default();
    }
    let pairs = take_config_secrets(cfg);
    let mut moved = 0usize;
    for (account, value) in pairs {
        if value.is_empty() {
            continue;
        }
        match secrets::set(AI_KEYS, &account, &value) {
            Ok(()) => moved += 1,
            Err(e) => {
                tracing::warn!("[ai] could not move {} into the store: {}", account, e);
                return Report::default();
            }
        }
    }
    let backup = match write_backup(path) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("[ai] backup failed, keeping the file as it is: {}", e);
            return Report::default();
        }
    };
    let Ok(json) = serde_json::to_string_pretty(&*cfg) else {
        return Report::default();
    };
    if let Err(e) = secrets::write_private(path, json.as_bytes()) {
        tracing::warn!("[ai] could not rewrite the config: {}", e);
        return Report::default();
    }
    tracing::info!(
        "[ai] moved {} key(s) out of {} into the secret store (backup at {})",
        moved,
        path.display(),
        backup.display()
    );
    Report {
        moved,
        backup: Some(backup),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_carry_the_provider_and_the_entry() {
        assert_eq!(key_account("openai", "abc"), "openai:abc");
        assert_eq!(token_account("newapi", "abc"), "newapi:abc#access_token");
    }

    #[test]
    fn taking_secrets_blanks_the_entry() {
        let mut entries = vec![KeyEntry {
            id: "e1".into(),
            kind: "openai".into(),
            key: " sk-live ".into(),
            access_token: String::new(),
            ..Default::default()
        }];
        assert!(entry_has_plaintext(&entries[0]));
        let pairs = take_secrets(&mut entries);
        assert_eq!(pairs[0], ("openai:e1".to_string(), "sk-live".to_string()));
        assert_eq!(pairs[1].1, "");
        assert!(entries[0].key.is_empty());
        assert!(!entry_has_plaintext(&entries[0]));
        // ...and the JSON that goes to disk no longer holds it.
        let json = serde_json::to_string(&entries).unwrap();
        assert!(!json.contains("sk-live"), "{json}");
    }

    #[test]
    fn the_backup_sits_next_to_the_file() {
        assert_eq!(
            backup_path(Path::new("/a/b/ai-keys.json")),
            PathBuf::from("/a/b/ai-keys.json.bak")
        );
    }
}
