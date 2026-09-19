//! `profile.json`: the part of the profile that is not a secret — nickname,
//! skin and creation time. The seed never comes near this file.
//!
//! It sits in the same folder as the profile's encrypted secret store
//! (`<app_data>/profile/`), so one directory holds the whole identity and one
//! environment override moves all of it in tests. Writes are tmp + rename, the
//! same shape `core/ai.rs` uses: a half-written `profile.json` would cost the
//! user their nickname on the next launch.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use unicode_normalization::UnicodeNormalization;

use crate::secrets::{self, PROFILE};

pub const FILE_NAME: &str = "profile.json";
pub const ERR_NICKNAME: &str = "ERR_PROFILE_NICKNAME";
pub const ERR_STORE: &str = "ERR_PROFILE_STORE";

pub const DEFAULT_NICKNAME: &str = "Omni";
pub const DEFAULT_SKIN_ID: &str = "omni-default";
/// The roster's default tint. Kept here, not in the UI, so a profile created
/// before the picker exists is already a valid roster entry.
pub const DEFAULT_TINT: [u8; 3] = [110, 139, 255];

pub const NICKNAME_MAX_CHARS: usize = 32;
pub const SKIN_ID_MAX_CHARS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skin {
    pub id: String,
    pub tint: [u8; 3],
}

impl Default for Skin {
    fn default() -> Self {
        Self {
            id: DEFAULT_SKIN_ID.to_string(),
            tint: DEFAULT_TINT,
        }
    }
}

/// What lands on disk. `created_at_ms` is the profile's birthday, not the
/// file's mtime, so it survives a copy of the data directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileFile {
    pub nickname: String,
    #[serde(default)]
    pub skin: Skin,
    pub created_at_ms: u64,
}

impl ProfileFile {
    pub fn new(now_ms: u64) -> Self {
        Self {
            nickname: DEFAULT_NICKNAME.to_string(),
            skin: Skin::default(),
            created_at_ms: now_ms,
        }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn dir() -> Result<PathBuf, String> {
    secrets::base_dir(PROFILE).map_err(|e| format!("{ERR_STORE}: {e}"))
}

pub fn path() -> Result<PathBuf, String> {
    Ok(dir()?.join(FILE_NAME))
}

/// Trim, NFC-normalise, then check. WHY NFC: "é" typed as e + combining accent
/// and "é" as one code point look identical and must not be two different
/// nicknames; NFC also makes the 32-character limit mean what the user sees.
pub fn normalize_nickname(raw: &str) -> Result<String, String> {
    let normalized: String = raw.trim().nfc().collect();
    if normalized.is_empty() {
        return Err(format!("{ERR_NICKNAME}: a nickname cannot be empty"));
    }
    if normalized.chars().any(|c| c.is_control()) {
        return Err(format!(
            "{ERR_NICKNAME}: a nickname cannot contain control characters"
        ));
    }
    let len = normalized.chars().count();
    if len > NICKNAME_MAX_CHARS {
        return Err(format!(
            "{ERR_NICKNAME}: a nickname is at most {NICKNAME_MAX_CHARS} characters, this one has {len}"
        ));
    }
    Ok(normalized)
}

/// Skin ids name files in the atlas, so they stay inside a character set that
/// cannot walk out of a directory or surprise a case-insensitive filesystem.
pub fn normalize_skin_id(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Err(format!("{ERR_STORE}: a skin id cannot be empty"));
    }
    if trimmed.chars().count() > SKIN_ID_MAX_CHARS {
        return Err(format!(
            "{ERR_STORE}: a skin id is at most {SKIN_ID_MAX_CHARS} characters"
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "{ERR_STORE}: a skin id may only use letters, digits, '-' and '_'"
        ));
    }
    Ok(trimmed)
}

/// Read `profile.json`. A missing file is not an error — it is a machine that
/// has not made a profile yet. A corrupt one is: overwriting it silently would
/// throw away a nickname the user picked, so the caller decides.
pub fn load() -> Result<Option<ProfileFile>, String> {
    let path = path()?;
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "{ERR_STORE}: could not read {}: {e}",
                path.display()
            ))
        }
    };
    let file: ProfileFile = serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "{ERR_STORE}: {} is not a valid profile: {e}",
            path.display()
        )
    })?;
    Ok(Some(file))
}

pub fn save(file: &ProfileFile) -> Result<(), String> {
    let dir = dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("{ERR_STORE}: could not create {}: {e}", dir.display()))?;
    let path = dir.join(FILE_NAME);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(file)
        .map_err(|e| format!("{ERR_STORE}: could not encode the profile: {e}"))?;
    if let Err(e) = std::fs::write(&tmp, &bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "{ERR_STORE}: could not write {}: {e}",
            tmp.display()
        ));
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "{ERR_STORE}: could not replace {}: {e}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::test_env::EnvGuard;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "omniget-profile-store-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn nicknames_are_trimmed_normalized_and_bounded() {
        assert_eq!(normalize_nickname("  Tonho  ").unwrap(), "Tonho");

        // e + U+0301 becomes the single code point é, so the two spellings are
        // one nickname.
        let decomposed = "Andre\u{0301}";
        let composed = "Andr\u{00e9}";
        assert_eq!(normalize_nickname(decomposed).unwrap(), composed);
        assert_eq!(
            normalize_nickname(decomposed).unwrap(),
            normalize_nickname(composed).unwrap()
        );

        for bad in ["", "   ", "ab\ncd", "tab\there", "\u{0007}"] {
            let err = normalize_nickname(bad).unwrap_err();
            assert!(err.starts_with(ERR_NICKNAME), "{bad:?} -> {err}");
        }

        let at_limit: String = "a".repeat(NICKNAME_MAX_CHARS);
        assert_eq!(normalize_nickname(&at_limit).unwrap(), at_limit);
        let too_long: String = "a".repeat(NICKNAME_MAX_CHARS + 1);
        assert!(normalize_nickname(&too_long)
            .unwrap_err()
            .starts_with(ERR_NICKNAME));

        // Emoji are characters, not bytes: 32 of them fit.
        let emoji: String = "\u{1f600}".repeat(NICKNAME_MAX_CHARS);
        assert_eq!(normalize_nickname(&emoji).unwrap(), emoji);
    }

    #[test]
    fn skin_ids_stay_inside_a_safe_alphabet() {
        assert_eq!(normalize_skin_id(" Omni-Default ").unwrap(), "omni-default");
        for bad in ["", "../escape", "has space", "acento-é"] {
            let err = normalize_skin_id(bad).unwrap_err();
            assert!(err.starts_with(ERR_STORE), "{bad:?} -> {err}");
        }
    }

    #[test]
    fn profile_json_round_trips_and_leaves_no_tmp_behind() {
        let dir = temp_dir("roundtrip");
        let guard = EnvGuard::acquire();
        guard.set(PROFILE.dir_env, Some(dir.to_str().expect("utf-8 path")));

        assert_eq!(load().unwrap(), None);
        let mut file = ProfileFile::new(1_700_000_000_000);
        file.nickname = "Tonho".into();
        file.skin = Skin {
            id: "omni-mint".into(),
            tint: [1, 2, 3],
        };
        save(&file).unwrap();
        assert_eq!(load().unwrap().as_ref(), Some(&file));

        // Overwrite works and the temporary file does not survive it.
        file.nickname = "Outro".into();
        save(&file).unwrap();
        assert_eq!(load().unwrap().as_ref(), Some(&file));
        assert!(!dir.join("profile.json.tmp").exists());

        // The seed is not in there.
        let raw = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(!raw.contains("seed"), "{raw}");

        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_profile_is_an_error_not_a_silent_reset() {
        let dir = temp_dir("corrupt");
        let guard = EnvGuard::acquire();
        guard.set(PROFILE.dir_env, Some(dir.to_str().expect("utf-8 path")));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), b"{ not json").unwrap();
        let err = load().unwrap_err();
        assert!(err.starts_with(ERR_STORE), "{err}");
        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file written before the skin field existed still loads.
    #[test]
    fn an_old_profile_without_a_skin_gets_the_default_one() {
        let dir = temp_dir("legacy");
        let guard = EnvGuard::acquire();
        guard.set(PROFILE.dir_env, Some(dir.to_str().expect("utf-8 path")));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(FILE_NAME),
            br#"{"nickname":"Tonho","created_at_ms":7}"#,
        )
        .unwrap();
        let file = load().unwrap().expect("profile");
        assert_eq!(file.skin, Skin::default());
        assert_eq!(file.created_at_ms, 7);
        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
