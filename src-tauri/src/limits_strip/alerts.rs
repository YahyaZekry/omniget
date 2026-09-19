//! Threshold crossings and "limit reset", as pure functions of two readings.
//! The engine turns what comes out into a notification and a
//! `limits://chime` event; nothing here touches a clock or the app.

use super::{LimitWindow, Reading};
use serde::Serialize;

/// A reset only counts when the window had something to give back.
const RESET_MIN_DROP: f32 = 0.10;
/// Slack when comparing two `resets_at` of the same window.
const RESET_SLACK_MS: i64 = 60_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Alert {
    /// `percent` is the threshold that was crossed, not the usage.
    Threshold {
        window: String,
        label: String,
        percent: u8,
    },
    Reset {
        window: String,
        label: String,
    },
}

/// The highest threshold that sits in `(prev, next]`.
pub fn crossed(prev: f32, next: f32, thresholds: &[u8]) -> Option<u8> {
    thresholds
        .iter()
        .copied()
        .filter(|t| {
            let t = f32::from(*t) / 100.0;
            prev < t && next >= t
        })
        .max()
}

/// The window emptied because its period rolled over, not because a rolling
/// average drifted down: usage fell by a real amount and either the old reset
/// time has passed or the provider now names a later one.
pub fn was_reset(prev: &LimitWindow, next: &LimitWindow, now_ms: i64) -> bool {
    let (Some(before), Some(after)) = (prev.used, next.used) else {
        return false;
    };
    if before - after < RESET_MIN_DROP {
        return false;
    }
    match (prev.resets_at, next.resets_at) {
        (Some(old), Some(new)) => old <= now_ms || new > old + RESET_SLACK_MS,
        (Some(old), None) => old <= now_ms,
        _ => false,
    }
}

/// What changed between two readings of one provider. A first reading raises
/// nothing: opening the strip at 90% is not news, going from 79% to 81% is.
pub fn between(
    prev: Option<&Reading>,
    next: &Reading,
    thresholds: &[u8],
    now_ms: i64,
) -> Vec<Alert> {
    let Some(prev) = prev else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for w in &next.windows {
        let Some(old) = prev.windows.iter().find(|p| p.id == w.id) else {
            continue;
        };
        if was_reset(old, w, now_ms) {
            out.push(Alert::Reset {
                window: w.id.clone(),
                label: w.label.clone(),
            });
        } else if let (Some(a), Some(b)) = (old.used, w.used) {
            if let Some(percent) = crossed(a, b, thresholds) {
                out.push(Alert::Threshold {
                    window: w.id.clone(),
                    label: w.label.clone(),
                    percent,
                });
            }
        }
    }
    out
}

/// Drops what the user asked not to hear about.
pub fn wanted(alerts: Vec<Alert>, thresholds_on: bool, reset_on: bool) -> Vec<Alert> {
    alerts
        .into_iter()
        .filter(|a| match a {
            Alert::Threshold { .. } => thresholds_on,
            Alert::Reset { .. } => reset_on,
        })
        .collect()
}

/// Title and body of the notification. English on purpose: it is raised by the
/// backend, which has no locale; the strip's own pulse carries no words.
pub fn notification(provider_label: &str, alert: &Alert) -> (String, String) {
    match alert {
        Alert::Threshold { label, percent, .. } => (
            format!("{provider_label}: {percent}% used"),
            format!("{label} passed {percent}%."),
        ),
        Alert::Reset { label, .. } => (
            format!("{provider_label}: limit reset"),
            format!("{label} is fresh again."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(id: &str, pct: f64, resets_at: Option<i64>) -> Reading {
        Reading {
            windows: vec![LimitWindow::percent(id, "5h limit", pct).resets(resets_at)],
            ..Default::default()
        }
    }

    #[test]
    fn only_a_crossing_counts_and_the_highest_one_wins() {
        let t = [80, 95];
        assert_eq!(crossed(0.79, 0.81, &t), Some(80));
        assert_eq!(crossed(0.81, 0.90, &t), None);
        assert_eq!(crossed(0.50, 0.97, &t), Some(95));
        assert_eq!(crossed(0.95, 0.95, &t), None);
        assert_eq!(crossed(0.90, 0.80, &t), None);
    }

    #[test]
    fn a_first_reading_is_not_news() {
        assert!(between(None, &reading("session", 99.0, None), &[80, 95], 0).is_empty());
    }

    #[test]
    fn crossing_eighty_raises_one_threshold_alert() {
        let a = reading("session", 79.0, Some(10_000));
        let b = reading("session", 82.0, Some(10_000));
        assert_eq!(
            between(Some(&a), &b, &[80, 95], 5_000),
            vec![Alert::Threshold {
                window: "session".into(),
                label: "5h limit".into(),
                percent: 80
            }]
        );
        assert!(between(Some(&b), &b, &[80, 95], 5_000).is_empty());
    }

    #[test]
    fn a_rollover_is_a_reset_and_a_drift_is_not() {
        let full = reading("session", 90.0, Some(10_000));
        let fresh_later = reading("session", 2.0, Some(10_000 + 18_000_000));
        assert_eq!(
            between(Some(&full), &fresh_later, &[80], 9_000),
            vec![Alert::Reset {
                window: "session".into(),
                label: "5h limit".into()
            }]
        );
        // Same reset time still ahead, usage merely lower: a rolling window.
        let drifted = reading("session", 70.0, Some(10_000));
        assert!(between(Some(&full), &drifted, &[80], 9_000).is_empty());
        // The old reset time has passed.
        assert!(was_reset(&full.windows[0], &drifted.windows[0], 10_001));
        // A tiny dip is never a reset.
        let dip = reading("session", 85.0, Some(99_999_999));
        assert!(!was_reset(&full.windows[0], &dip.windows[0], 10_001));
    }

    #[test]
    fn muted_kinds_are_dropped() {
        let all = vec![
            Alert::Reset {
                window: "w".into(),
                label: "W".into(),
            },
            Alert::Threshold {
                window: "w".into(),
                label: "W".into(),
                percent: 95,
            },
        ];
        assert_eq!(wanted(all.clone(), true, false).len(), 1);
        assert_eq!(wanted(all.clone(), false, false).len(), 0);
        let (title, body) = notification("Codex", &all[1]);
        assert_eq!(title, "Codex: 95% used");
        assert_eq!(body, "W passed 95%.");
    }
}
