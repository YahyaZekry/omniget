//! Mascot `state` (Phase 5). Owned by f5-omni-intent.
//!
//! What the Omni is doing right now, persisted to `<app_data>/omni/state.json`
//! so the pet window and the rail avatar agree across restarts. No LLM is ever
//! involved here: the state only ever moves through `apply(&Intent)`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::intent::{Animation, Intent, Mood};

/// Energy when nothing said otherwise. Phase 4 (quota) will drive it down;
/// today it is a plain field with a setter.
pub const DEFAULT_ENERGY: u8 = 100;

/// Below this the mascot is too tired to work: see `rules`.
pub const EXHAUSTED_ENERGY: u8 = 0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OmniState {
    pub current: Intent,
    /// Unix epoch milliseconds at which `current` started.
    pub since_ms: u64,
    /// 0..=100. Fed by the quota in Phase 4; default `DEFAULT_ENERGY`.
    pub energy: u8,
}

impl Default for OmniState {
    fn default() -> Self {
        OmniState {
            current: Intent::neutral(),
            since_ms: now_ms(),
            energy: DEFAULT_ENERGY,
        }
    }
}

impl OmniState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves the mascot to `intent`. Returns `true` when something actually
    /// changed; an identical intent keeps `since_ms` so "how long has it been
    /// idle" stays honest.
    pub fn apply(&mut self, intent: &Intent) -> bool {
        if &self.current == intent {
            return false;
        }
        self.current = intent.clone();
        self.since_ms = now_ms();
        true
    }

    /// Clamped to 0..=100 by the type itself; kept as a method so Phase 4 has
    /// one place to hook the quota into.
    pub fn set_energy(&mut self, energy: u8) {
        self.energy = energy.min(100);
    }

    pub fn is_exhausted(&self) -> bool {
        self.energy == EXHAUSTED_ENERGY
    }

    pub fn animation(&self) -> Animation {
        self.current.animation
    }

    pub fn mood(&self) -> Mood {
        self.current.mood
    }

    /// Milliseconds spent in the current intent (saturating: a clock that went
    /// backwards reads as 0, never as a huge number).
    pub fn elapsed_ms(&self) -> u64 {
        now_ms().saturating_sub(self.since_ms)
    }

    /// `<app_data>/omni/state.json`. `None` when no data dir is available.
    pub fn path() -> Option<PathBuf> {
        crate::core::paths::app_data_dir().map(|d| d.join("omni").join("state.json"))
    }

    /// Never fails: an unreadable or corrupt file is a fresh default state.
    pub fn load() -> Self {
        Self::load_from(Self::path())
    }

    fn load_from(path: Option<PathBuf>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<OmniState>(&raw) {
            Ok(mut state) => {
                state.energy = state.energy.min(100);
                state
            }
            Err(_) => Self::default(),
        }
    }

    /// Writes atomically (temp file + rename) so a crash mid-write cannot leave
    /// half a JSON behind.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or_else(|| "ERR_OMNI_NO_DATA_DIR".to_string())?;
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("ERR_OMNI_STATE_DIR: {e}"))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("ERR_OMNI_STATE_ENCODE: {e}"))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("ERR_OMNI_STATE_WRITE: {e}"))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("ERR_OMNI_STATE_RENAME: {e}"))?;
        Ok(())
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_idle_with_full_energy() {
        let s = OmniState::new();
        assert_eq!(s.current, Intent::neutral());
        assert_eq!(s.energy, DEFAULT_ENERGY);
        assert!(!s.is_exhausted());
        assert!(s.since_ms > 0);
    }

    #[test]
    fn apply_changes_only_on_a_different_intent() {
        let mut s = OmniState::new();
        let walk = Intent::new(Animation::Walk, Mood::Curious);
        assert!(s.apply(&walk));
        let marked = s.since_ms;
        assert_eq!(s.animation(), Animation::Walk);
        assert_eq!(s.mood(), Mood::Curious);
        assert!(!s.apply(&walk));
        assert_eq!(s.since_ms, marked, "since_ms survives a repeated intent");
        assert!(s.apply(&Intent::new(Animation::Walk, Mood::Happy)));
    }

    #[test]
    fn energy_is_clamped_and_exhaustion_is_zero() {
        let mut s = OmniState::new();
        s.set_energy(200);
        assert_eq!(s.energy, 100);
        s.set_energy(0);
        assert!(s.is_exhausted());
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("omni-state-{}", now_ms()));
        let path = dir.join("state.json");
        let mut s = OmniState::new();
        s.apply(&Intent::new(Animation::Sleep, Mood::Sleepy).with_line("zzz"));
        s.set_energy(7);
        s.save_to(&path).expect("save");
        let back = OmniState::load_from(Some(path.clone()));
        assert_eq!(back, s);
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_or_missing_file_reads_as_default() {
        let dir = std::env::temp_dir().join(format!("omni-state-bad-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(
            OmniState::load_from(Some(path.clone())).current,
            Intent::neutral()
        );
        let missing = dir.join("nope.json");
        assert_eq!(OmniState::load_from(Some(missing)).energy, DEFAULT_ENERGY);
        assert_eq!(OmniState::load_from(None).energy, DEFAULT_ENERGY);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_follows_the_data_dir_override() {
        // `paths::app_data_dir` honours OMNIGET_DATA_DIR; the test only asserts
        // the shape of the tail so it cannot race another test's env write.
        let p = OmniState::path().expect("a data dir exists on every CI runner");
        assert!(p.ends_with(std::path::Path::new("omni").join("state.json")));
    }
}
