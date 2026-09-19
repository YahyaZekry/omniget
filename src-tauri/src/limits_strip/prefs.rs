//! `limits-strip.json`: everything the user chose about the limits strip. Lives
//! in its own file under `<data>/llm/limits/`, written through a temp file and
//! a rename. No secret is ever written here: the file holds switches only.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const PREFS_FILE: &str = "limits-strip.json";
/// Moves the whole folder, for tests and for a portable install.
pub const DIR_ENV: &str = "OMNIGET_LIMITS_DIR";

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default, PartialOrd, Ord,
)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

impl Edge {
    pub fn as_str(self) -> &'static str {
        match self {
            Edge::Top => "top",
            Edge::Bottom => "bottom",
            Edge::Left => "left",
            Edge::Right => "right",
        }
    }

    pub fn parse(raw: &str) -> Option<Edge> {
        match raw {
            "top" => Some(Edge::Top),
            "bottom" => Some(Edge::Bottom),
            "left" => Some(Edge::Left),
            "right" => Some(Edge::Right),
            _ => None,
        }
    }

    pub fn is_vertical(self) -> bool {
        matches!(self, Edge::Left | Edge::Right)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderPref {
    pub id: String,
    /// Off = never read, never polled, readings forgotten.
    pub enabled: bool,
    /// No threshold or reset notification for this one.
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StripPrefs {
    /// Master switch. OFF by default: nothing is read and no window exists
    /// until the user asks. It also reopens the strip on the next launch.
    pub enabled: bool,
    pub edge: Edge,
    /// Where along each edge the strip sits, 0..1 (0.5 = centred). Each edge
    /// remembers its own spot.
    pub along: BTreeMap<Edge, f64>,
    /// Order and switches. Providers found later join the end.
    pub providers: Vec<ProviderPref>,
    pub notify_thresholds: bool,
    /// Percentages, ascending.
    pub thresholds: Vec<u8>,
    pub notify_reset: bool,
}

impl Default for StripPrefs {
    fn default() -> Self {
        Self {
            enabled: false,
            edge: Edge::Top,
            along: BTreeMap::new(),
            providers: Vec::new(),
            notify_thresholds: true,
            thresholds: vec![80, 95],
            notify_reset: true,
        }
    }
}

impl StripPrefs {
    pub fn along_of(&self, edge: Edge) -> f64 {
        self.along
            .get(&edge)
            .copied()
            .unwrap_or(0.5)
            .clamp(0.0, 1.0)
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderPref> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// A provider the file does not list is OFF: a reader is only ever run
    /// because the user ticked its box.
    pub fn is_on(&self, id: &str) -> bool {
        self.enabled && self.provider(id).map(|p| p.enabled).unwrap_or(false)
    }

    pub fn is_muted(&self, id: &str) -> bool {
        self.provider(id).map(|p| p.muted).unwrap_or(false)
    }

    /// Adds the providers the list does not know yet at the end, so nothing
    /// the user placed by hand is overtaken by a newcomer. `known` pairs each
    /// id with its default: only what stays on this machine starts ticked.
    pub fn adopt(&mut self, known: &[(&str, bool)]) -> bool {
        let mut changed = false;
        for (id, on) in known {
            if self.provider(id).is_none() {
                self.providers.push(ProviderPref {
                    id: (*id).to_string(),
                    enabled: *on,
                    muted: false,
                });
                changed = true;
            }
        }
        let before = self.providers.len();
        self.providers
            .retain(|p| known.iter().any(|(id, _)| *id == p.id));
        changed || self.providers.len() != before
    }

    /// Thresholds cleaned up: 1..=100, ascending, no repeats, at most four.
    pub fn sane_thresholds(&self) -> Vec<u8> {
        let mut t: Vec<u8> = self
            .thresholds
            .iter()
            .copied()
            .filter(|v| (1..=100).contains(v))
            .collect();
        t.sort_unstable();
        t.dedup();
        t.truncate(4);
        t
    }
}

pub fn prefs_from_str(raw: &str) -> StripPrefs {
    serde_json::from_str(raw).unwrap_or_default()
}

pub fn dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(DIR_ENV) {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    crate::core::paths::app_data_dir().map(|d| d.join("llm").join("limits"))
}

pub fn load() -> StripPrefs {
    let Some(dir) = dir() else {
        return StripPrefs::default();
    };
    match std::fs::read_to_string(dir.join(PREFS_FILE)) {
        Ok(raw) => prefs_from_str(&raw),
        Err(_) => StripPrefs::default(),
    }
}

pub fn save(prefs: &StripPrefs) -> Result<(), String> {
    let dir = dir().ok_or("ERR_LIMITS_STORE: no data directory")?;
    save_in(&dir, prefs)
}

pub fn save_in(dir: &std::path::Path, prefs: &StripPrefs) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("ERR_LIMITS_STORE: {e}"))?;
    let body = serde_json::to_vec_pretty(prefs).map_err(|e| format!("ERR_LIMITS_STORE: {e}"))?;
    let tmp = dir.join(format!("{PREFS_FILE}.tmp"));
    std::fs::write(&tmp, &body).map_err(|e| format!("ERR_LIMITS_STORE: {e}"))?;
    std::fs::rename(&tmp, dir.join(PREFS_FILE)).map_err(|e| format!("ERR_LIMITS_STORE: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_is_off_until_someone_switches_it_on() {
        let p = StripPrefs::default();
        assert!(!p.enabled);
        assert!(p.providers.is_empty());
        assert_eq!(p.thresholds, vec![80, 95]);
    }

    #[test]
    fn a_provider_is_read_only_with_both_switches_on() {
        let mut p = StripPrefs::default();
        p.adopt(&[("claude", false), ("ollama", true)]);
        assert!(!p.is_on("claude") && !p.is_on("ollama"), "master is off");
        p.enabled = true;
        assert!(!p.is_on("claude"), "remote readers start unticked");
        assert!(p.is_on("ollama"));
        assert!(!p.is_on("never-listed"));
        p.providers[0].enabled = true;
        assert!(p.is_on("claude"));
    }

    #[test]
    fn a_corrupt_file_costs_the_placement_not_the_launch() {
        assert_eq!(prefs_from_str("{not json"), StripPrefs::default());
        let p = prefs_from_str(r#"{"enabled":true,"edge":"left","along":{"left":0.2}}"#);
        assert!(p.enabled);
        assert_eq!(p.edge, Edge::Left);
        assert!((p.along_of(Edge::Left) - 0.2).abs() < 1e-9);
        assert!((p.along_of(Edge::Top) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn newcomers_join_the_end_and_retired_readers_leave() {
        let mut p = StripPrefs::default();
        p.providers.push(ProviderPref {
            id: "codex".into(),
            enabled: true,
            muted: false,
        });
        p.providers.push(ProviderPref {
            id: "retired".into(),
            ..Default::default()
        });
        assert!(p.adopt(&[("claude", false), ("codex", false), ("cursor", false)]));
        let ids: Vec<_> = p.providers.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["codex", "claude", "cursor"]);
        assert!(p.providers[0].enabled, "the user's tick survives");
        assert!(!p.adopt(&[("claude", false), ("codex", false), ("cursor", false)]));
    }

    #[test]
    fn thresholds_are_cleaned_before_use() {
        let p = StripPrefs {
            thresholds: vec![95, 0, 80, 80, 101, 50, 60, 70],
            ..Default::default()
        };
        assert_eq!(p.sane_thresholds(), vec![50, 60, 70, 80]);
    }

    #[test]
    fn prefs_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("omniget-limits-prefs-{}", std::process::id()));
        let mut p = StripPrefs {
            enabled: true,
            edge: Edge::Right,
            ..Default::default()
        };
        p.along.insert(Edge::Right, 0.25);
        save_in(&dir, &p).unwrap();
        let raw = std::fs::read_to_string(dir.join(PREFS_FILE)).unwrap();
        assert_eq!(prefs_from_str(&raw), p);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
