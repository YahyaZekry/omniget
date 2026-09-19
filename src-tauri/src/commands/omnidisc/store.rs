//! Session tokens, one per instance URL.
//!
//! The storage itself lives in `crate::secrets`, which is namespace-neutral and
//! shared with the local profile. This module is the OmniDisc view of it: it
//! binds every call to [`crate::secrets::OMNIDISC`] and keeps the names the
//! rest of the OmniDisc client already uses. Behaviour is unchanged — same
//! keyring service, same folder, same `OMNIGET_OMNIDISC_SESSION_DIR` override,
//! same AES-256-CBC + HMAC-SHA256 file — see the module docs of `secrets.rs`
//! for where the bytes end up and why.
//!
//! Tokens never leave this module towards the frontend.

use crate::secrets::{self, OMNIDISC};

pub use crate::secrets::{write_private, FileStore};

pub const SERVICE: &str = OMNIDISC.service;
pub const SESSION_DIR_ENV: &str = OMNIDISC.dir_env;

/// Directory the file store writes into. Also where the MLS state blobs live,
/// so the integration test's `OMNIGET_OMNIDISC_SESSION_DIR` moves every OmniDisc
/// secret at once instead of half of them.
pub fn base_dir() -> Result<std::path::PathBuf, String> {
    secrets::base_dir(OMNIDISC)
}

/// Secrets other than the session token (device key, MLS state key) share the
/// same store under a suffixed account so one instance's material stays together
/// and is dropped together on logout.
pub fn secret_account(url: &str, kind: &str) -> String {
    secrets::secret_account(url, kind)
}

pub fn save_secret(account: &str, value: &str) -> Result<(), String> {
    secrets::save_secret(OMNIDISC, account, value)
}

pub fn load_secret(account: &str) -> Result<Option<String>, String> {
    secrets::load_secret(OMNIDISC, account)
}

pub fn delete_secret(account: &str) -> Result<(), String> {
    secrets::delete_secret(OMNIDISC, account)
}

pub fn save_token(url: &str, token: &str) -> Result<(), String> {
    save_secret(url, token)
}

pub fn load_token(url: &str) -> Result<Option<String>, String> {
    load_secret(url)
}

pub fn delete_token(url: &str) -> Result<(), String> {
    delete_secret(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{decrypt, encrypt, IV_LEN, SESSIONS_FILE};
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "omnidisc-store-{}-{}-{}",
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
    fn file_store_roundtrip() {
        let dir = temp_dir("roundtrip");
        let store = FileStore::new(&dir);
        assert_eq!(store.get("https://a.example").unwrap(), None);
        store.set("https://a.example", "od1.secret-a").unwrap();
        store.set("https://b.example", "od1.secret-b").unwrap();
        assert_eq!(
            store.get("https://a.example").unwrap().as_deref(),
            Some("od1.secret-a")
        );
        assert_eq!(
            store.get("https://b.example").unwrap().as_deref(),
            Some("od1.secret-b")
        );
        store.remove("https://a.example").unwrap();
        assert_eq!(store.get("https://a.example").unwrap(), None);
        assert_eq!(
            store.get("https://b.example").unwrap().as_deref(),
            Some("od1.secret-b")
        );
        let raw = std::fs::read(dir.join(SESSIONS_FILE)).unwrap();
        assert!(!raw
            .windows(b"od1.secret-b".len())
            .any(|w| w == b"od1.secret-b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tampering_is_rejected() {
        let dir = temp_dir("tamper");
        let store = FileStore::new(&dir);
        store.set("https://a.example", "od1.secret").unwrap();
        let path = dir.join(SESSIONS_FILE);
        let mut raw = std::fs::read(&path).unwrap();
        let idx = IV_LEN + 1;
        raw[idx] ^= 0xff;
        std::fs::write(&path, raw).unwrap();
        assert!(store.get("https://a.example").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The session key is the key to every stored token: it must never exist on
    /// disk, not even for an instant, in a mode another local account can read.
    #[cfg(unix)]
    #[test]
    fn private_files_are_never_readable_by_anyone_else() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("private");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.join("key.bin");
        write_private(&path, b"first").unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(
            mode(&dir),
            0o700,
            "the folder around it has to be private too"
        );

        // An overwrite keeps the mode, and a file an older build left open is
        // tightened before it is written again.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"second").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn encrypt_decrypt_roundtrip_and_mac() {
        let key = [7u8; 32];
        let blob = encrypt(&key, b"hello");
        assert_eq!(decrypt(&key, &blob).unwrap(), b"hello");
        let other = [8u8; 32];
        assert!(decrypt(&other, &blob).is_err());
    }

    /// The re-export is the point of this module: the OmniDisc names have to
    /// keep resolving to the OmniDisc namespace after the extraction.
    #[test]
    fn omnidisc_names_still_point_at_the_omnidisc_namespace() {
        assert_eq!(SERVICE, "wtf.tonho.omniget.omnidisc");
        assert_eq!(SESSION_DIR_ENV, "OMNIGET_OMNIDISC_SESSION_DIR");
        assert_eq!(
            secret_account("https://a.example", "device-key"),
            "https://a.example#device-key"
        );

        let dir = temp_dir("namespace");
        let root = temp_dir("namespace-root");
        let guard = crate::secrets::test_env::EnvGuard::acquire();
        guard.set(
            crate::secrets::SECRETS_DIR_ENV,
            Some(root.to_str().expect("utf-8 path")),
        );
        guard.set(crate::secrets::PROFILE.dir_env, None);
        guard.set(SESSION_DIR_ENV, Some(dir.to_str().expect("utf-8 path")));
        assert_eq!(base_dir().unwrap(), dir);
        save_token("https://a.example", "od1.token").unwrap();
        crate::secrets::clear_cache();
        assert_eq!(
            load_token("https://a.example").unwrap().as_deref(),
            Some("od1.token")
        );
        // ...and the profile namespace does not see it.
        assert_eq!(
            crate::secrets::load_secret(crate::secrets::PROFILE, "https://a.example").unwrap(),
            None
        );
        delete_token("https://a.example").unwrap();
        crate::secrets::clear_cache();
        assert_eq!(load_token("https://a.example").unwrap(), None);
        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&root);
    }
}
