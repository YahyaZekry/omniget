//! The local profile: an ed25519 identity, a nickname and a skin. No account,
//! no e-mail, no password, no network.
//!
//! The public key is the identifier the multiplayer layer and `omnidisc-mls`
//! already expect, so the profile is the one place in the app that answers "who
//! is this machine". Everything here is lazy: [`ProfileManager::new`] takes no
//! arguments and touches nothing, because `AppState` is built before the Tauri
//! app exists; the first call that needs the identity loads it and caches it,
//! and every call after that is a read lock and a clone.
//!
//! Error strings are stable codes the UI maps directly: `ERR_PROFILE_NICKNAME`,
//! `ERR_PROFILE_STORE`, `ERR_PROFILE_SECRET`, `ERR_PROFILE_SIGN`, optionally
//! followed by `": "` and a detail.

pub mod identity;
pub mod store;

use serde::Serialize;
use std::sync::RwLock;

pub use identity::{fingerprint, verify};
pub use identity::{ERR_SECRET, ERR_SIGN};
pub use store::{ProfileFile, Skin, ERR_NICKNAME, ERR_STORE};

/// The profile as the rest of the Rust side sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub public_key: [u8; 32],
    pub nickname: String,
    pub skin: Skin,
    pub created_at_ms: u64,
}

impl Profile {
    pub fn fingerprint(&self) -> String {
        identity::fingerprint(&self.public_key)
    }

    /// The shape the frontend gets. The key is base64 and the fingerprint comes
    /// precomputed so no caller has to reimplement the base32 grouping.
    pub fn view(&self) -> ProfileView {
        ProfileView {
            public_key_b64: identity::encode_public_key(&self.public_key),
            fingerprint: self.fingerprint(),
            nickname: self.nickname.clone(),
            skin: self.skin.clone(),
            created_at_ms: self.created_at_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProfileView {
    pub public_key_b64: String,
    pub fingerprint: String,
    pub nickname: String,
    pub skin: Skin,
    pub created_at_ms: u64,
}

/// What the frontend needs to introduce this machine to another one.
#[derive(Clone, Debug, Serialize)]
pub struct ProfilePublic {
    pub public_key_b64: String,
    pub fingerprint: String,
}

#[derive(Clone)]
struct Loaded {
    seed: [u8; 32],
    /// Derived from the seed once. Deriving it is a scalar multiplication, and
    /// `get` is meant to be a read lock and a clone.
    public_key: [u8; 32],
    file: ProfileFile,
}

impl Loaded {
    fn profile(&self) -> Profile {
        Profile {
            public_key: self.public_key,
            nickname: self.file.nickname.clone(),
            skin: self.file.skin.clone(),
            created_at_ms: self.file.created_at_ms,
        }
    }
}

#[derive(Default)]
pub struct ProfileManager {
    cached: RwLock<Option<Loaded>>,
}

impl ProfileManager {
    pub fn new() -> Self {
        Self {
            cached: RwLock::new(None),
        }
    }

    /// Run `f` against the loaded profile, loading it first if this is the
    /// first call. A poisoned lock is not a reason to lose the identity: the
    /// inner value is still valid, so it is taken as-is.
    fn with<T>(&self, f: impl FnOnce(&Loaded) -> T) -> Result<T, String> {
        if let Some(loaded) = self
            .cached
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            return Ok(f(loaded));
        }
        let mut guard = self.cached.write().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = Some(load_from_disk()?);
        }
        let loaded = guard.as_ref().expect("just loaded");
        Ok(f(loaded))
    }

    /// Mutate the stored half of the profile and persist it. The seed is never
    /// touched here.
    fn update(
        &self,
        f: impl FnOnce(&mut ProfileFile) -> Result<(), String>,
    ) -> Result<Profile, String> {
        let mut guard = self.cached.write().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = Some(load_from_disk()?);
        }
        let loaded = guard.as_mut().expect("just loaded");
        let mut candidate = loaded.file.clone();
        f(&mut candidate)?;
        // Disk first: if the write fails the cache must keep matching the file.
        store::save(&candidate)?;
        loaded.file = candidate;
        Ok(loaded.profile())
    }

    /// The whole profile, creating it on the first call.
    pub fn get(&self) -> Result<Profile, String> {
        self.with(|loaded| loaded.profile())
    }

    pub fn set_nickname(&self, nick: &str) -> Result<Profile, String> {
        let nickname = store::normalize_nickname(nick)?;
        self.update(move |file| {
            file.nickname = nickname;
            Ok(())
        })
    }

    pub fn set_skin(&self, id: &str, tint: [u8; 3]) -> Result<Profile, String> {
        let id = store::normalize_skin_id(id)?;
        self.update(move |file| {
            file.skin = Skin { id, tint };
            Ok(())
        })
    }

    /// Sign with the local identity. The seed never leaves this module.
    pub fn sign(&self, msg: &[u8]) -> Result<[u8; 64], String> {
        self.with(|loaded| identity::sign(&loaded.seed, msg))
    }

    pub fn public(&self) -> Result<ProfilePublic, String> {
        self.with(|loaded| ProfilePublic {
            public_key_b64: identity::encode_public_key(&loaded.public_key),
            fingerprint: identity::fingerprint(&loaded.public_key),
        })
    }

    /// Drop the cache so the next call reads disk again. Used by tests that
    /// move the profile directory around inside one process.
    pub fn reset_cache(&self) {
        *self.cached.write().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// Load the seed and `profile.json`, creating either one if it is missing.
fn load_from_disk() -> Result<Loaded, String> {
    let seed = identity::load_or_create_seed()?;
    let file = match store::load()? {
        Some(file) => file,
        None => {
            let file = ProfileFile::new(store::now_ms());
            store::save(&file)?;
            file
        }
    };
    Ok(Loaded {
        public_key: identity::public_key(&seed),
        seed,
        file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::test_env::EnvGuard;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "omniget-profile-{}-{}-{}",
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

    /// Point both namespaces at one empty root. Returned guard restores the
    /// environment and releases the global lock on drop.
    fn isolate(root: &std::path::Path) -> EnvGuard {
        let guard = EnvGuard::acquire();
        guard.set(
            crate::secrets::SECRETS_DIR_ENV,
            Some(root.to_str().expect("utf-8 path")),
        );
        guard.set(crate::secrets::PROFILE.dir_env, None);
        guard.set(crate::secrets::OMNIDISC.dir_env, None);
        // A developer machine with a signed build must not have this test write
        // into the real login keychain.
        guard.set(crate::secrets::KEYRING_ENV, Some("0"));
        guard.set(crate::secrets::KEYRING_ENV_LEGACY, None);
        guard
    }

    #[test]
    fn a_fresh_machine_gets_a_profile_that_signs_and_verifies() {
        let root = temp_dir("fresh");
        let guard = isolate(&root);

        let manager = ProfileManager::new();
        let start = std::time::Instant::now();
        let profile = manager.get().expect("profile");
        let first_get = start.elapsed();
        eprintln!("[budget] first profile_get (key generation included): {first_get:?}");
        assert!(
            first_get < std::time::Duration::from_millis(50),
            "generating the identity took {first_get:?}"
        );
        assert_eq!(profile.nickname, store::DEFAULT_NICKNAME);
        assert_eq!(profile.skin, Skin::default());
        assert!(profile.created_at_ms > 0);
        assert_ne!(profile.public_key, [0u8; 32]);

        let msg = b"hello from the world";
        let sig = manager.sign(msg).expect("sign");
        assert!(verify(&profile.public_key, msg, &sig));
        assert!(!verify(&profile.public_key, b"not this", &sig));

        // The view is what the frontend gets, and it agrees with the struct.
        let view = profile.view();
        assert_eq!(view.fingerprint, profile.fingerprint());
        assert_eq!(view.nickname, profile.nickname);
        assert_eq!(view.skin, profile.skin);

        // A second manager in the same directory is the same identity.
        let again = ProfileManager::new().get().expect("profile");
        assert_eq!(again.public_key, profile.public_key);
        assert_eq!(again.created_at_ms, profile.created_at_ms);

        drop(guard);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nickname_and_skin_survive_a_restart_and_bad_input_is_refused() {
        let root = temp_dir("edit");
        let guard = isolate(&root);

        let manager = ProfileManager::new();
        manager.get().expect("profile");
        let edited = manager.set_nickname("  Tonho  ").expect("nickname");
        assert_eq!(edited.nickname, "Tonho");
        let edited = manager.set_skin("Omni-Mint", [9, 8, 7]).expect("skin");
        assert_eq!(edited.skin.id, "omni-mint");
        assert_eq!(edited.skin.tint, [9, 8, 7]);

        let err = manager.set_nickname("   ").unwrap_err();
        assert!(err.starts_with(ERR_NICKNAME), "{err}");
        let err = manager.set_nickname(&"a".repeat(33)).unwrap_err();
        assert!(err.starts_with(ERR_NICKNAME), "{err}");
        let err = manager.set_nickname("bad\nnick").unwrap_err();
        assert!(err.starts_with(ERR_NICKNAME), "{err}");
        // A refused edit does not touch what is stored.
        assert_eq!(manager.get().unwrap().nickname, "Tonho");

        let reloaded = ProfileManager::new().get().expect("profile");
        assert_eq!(reloaded.nickname, "Tonho");
        assert_eq!(reloaded.skin.id, "omni-mint");

        drop(guard);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A machine that already ran OmniDisc must keep the key its instances and
    /// MLS groups know, instead of quietly becoming a second person.
    #[test]
    fn an_existing_omnidisc_device_key_becomes_the_profile_identity() {
        let root = temp_dir("migrate");
        let guard = isolate(&root);

        let seed = [23u8; 32];
        let url = "https://chat.example";
        let omnidisc_dir = crate::secrets::base_dir(crate::secrets::OMNIDISC).unwrap();
        std::fs::create_dir_all(&omnidisc_dir).unwrap();
        std::fs::write(
            omnidisc_dir.join("device-ids.json"),
            serde_json::to_vec(&serde_json::json!({ url: "od-1234abcd" })).unwrap(),
        )
        .unwrap();
        crate::secrets::save_secret(
            crate::secrets::OMNIDISC,
            &crate::secrets::secret_account(url, "device-key"),
            &identity::encode_seed(&seed),
        )
        .unwrap();

        let profile = ProfileManager::new().get().expect("profile");
        assert_eq!(profile.public_key, identity::public_key(&seed));

        // The OmniDisc copy is left alone, so the MLS side keeps working.
        assert_eq!(
            crate::secrets::load_secret(
                crate::secrets::OMNIDISC,
                &crate::secrets::secret_account(url, "device-key")
            )
            .unwrap()
            .as_deref(),
            Some(identity::encode_seed(&seed).as_str())
        );
        // And the profile has its own copy from now on.
        assert_eq!(
            crate::secrets::load_secret(crate::secrets::PROFILE, identity::SEED_ACCOUNT)
                .unwrap()
                .as_deref(),
            Some(identity::encode_seed(&seed).as_str())
        );

        drop(guard);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The same migration when the device key is where every published build
    /// before the signature-based policy put it: the OS keyring, with nothing
    /// in the file store. Without the last-resort read in `secrets::load_secret`
    /// the profile would generate a brand new identity here and orphan the
    /// user's MLS groups.
    #[test]
    fn a_device_key_that_only_exists_in_the_keyring_is_still_adopted() {
        let root = temp_dir("migrate-keyring");
        // Not `isolate`: the last-resort read only happens on the real path, so
        // the directories come from OMNIGET_DATA_DIR instead of a forced one.
        let guard = EnvGuard::acquire();
        guard.set("OMNIGET_DATA_DIR", Some(root.to_str().expect("utf-8 path")));
        guard.set(crate::secrets::SECRETS_DIR_ENV, None);
        guard.set(crate::secrets::PROFILE.dir_env, None);
        guard.set(crate::secrets::OMNIDISC.dir_env, None);
        guard.set(crate::secrets::KEYRING_ENV, Some("0"));
        guard.set(crate::secrets::KEYRING_ENV_LEGACY, None);

        let seed = [37u8; 32];
        let url = "https://chat.example";
        let account = crate::secrets::secret_account(url, "device-key");
        let omnidisc_dir = crate::secrets::base_dir(crate::secrets::OMNIDISC).unwrap();
        std::fs::create_dir_all(&omnidisc_dir).unwrap();
        std::fs::write(
            omnidisc_dir.join("device-ids.json"),
            serde_json::to_vec(&serde_json::json!({ url: "od-1234abcd" })).unwrap(),
        )
        .unwrap();
        // The seed exists only in the keyring: the file store is empty.
        crate::secrets::test_keyring::install(crate::secrets::test_keyring::Fake::release().with(
            crate::secrets::OMNIDISC.service,
            &account,
            &identity::encode_seed(&seed),
        ));
        assert!(crate::secrets::file_store_accounts(crate::secrets::OMNIDISC).is_empty());

        let profile = ProfileManager::new().get().expect("profile");
        assert_eq!(
            profile.public_key,
            identity::public_key(&seed),
            "the profile has to adopt the key the MLS groups already know"
        );
        // The keyring copy is untouched, and the OmniDisc side now has a file
        // copy too, so the next launch does not need the keychain at all.
        assert!(crate::secrets::test_keyring::holds(
            crate::secrets::OMNIDISC.service,
            &account
        ));
        assert_eq!(
            crate::secrets::file_store_accounts(crate::secrets::OMNIDISC),
            vec![account.clone()]
        );

        crate::secrets::test_keyring::clear();
        drop(guard);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// With the keyring refused (or absent, as on Linux) everything still
    /// works: the seed goes to the encrypted file store, and it is not in
    /// there in plain text.
    #[test]
    fn a_refused_keyring_falls_back_to_the_encrypted_file() {
        let root = temp_dir("fallback");
        let guard = isolate(&root);
        assert!(
            !crate::secrets::use_keyring(),
            "OMNIGET_KEYRING=0 has to turn the keyring off on every platform"
        );

        ProfileManager::new().get().expect("profile");
        let store_file = root.join("profile").join(crate::secrets::SESSIONS_FILE);
        assert!(store_file.exists(), "{}", store_file.display());
        let raw = std::fs::read(&store_file).unwrap();
        let stored = crate::secrets::load_secret(crate::secrets::PROFILE, identity::SEED_ACCOUNT)
            .unwrap()
            .expect("the seed is in the file store");
        // Neither the seed nor the account name it is filed under is readable.
        assert!(!raw.windows(stored.len()).any(|w| w == stored.as_bytes()));
        assert!(!raw
            .windows(identity::SEED_ACCOUNT.len())
            .any(|w| w == identity::SEED_ACCOUNT.as_bytes()));

        drop(guard);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The budget: `profile_get` is a read lock and a clone once the identity
    /// is loaded. Generous bound because CI machines are noisy; the point is
    /// that nothing on this path reads a file or spawns a process.
    #[test]
    fn a_cached_get_costs_nothing() {
        let root = temp_dir("budget");
        let guard = isolate(&root);

        let manager = ProfileManager::new();
        manager.get().expect("profile");
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            manager.get().expect("profile");
        }
        let per_call = start.elapsed() / 1000;
        eprintln!("[budget] cached profile_get: {per_call:?} per call over 1000 calls");
        assert!(
            per_call < std::time::Duration::from_millis(1),
            "cached profile_get took {per_call:?} per call"
        );

        drop(guard);
        let _ = std::fs::remove_dir_all(&root);
    }
}
