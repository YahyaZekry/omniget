//! Codex.
//!
//! Credential (read only): `$CODEX_HOME/auth.json`, default `~/.codex/auth.json`
//! on every OS (`%USERPROFILE%\.codex` on Windows). Keys `tokens.access_token`
//! and `tokens.account_id`. A keychain-only or API-key-only login carries no
//! ChatGPT plan limits and reads as absent here.
//!
//! Live: `GET https://chatgpt.com/backend-api/wham/usage` with `Authorization:
//! Bearer` and `ChatGPT-Account-Id`. Reply: `rate_limit.{primary_window,
//! secondary_window}` with `used_percent`, `limit_window_seconds`, `reset_at`
//! (epoch seconds) or `reset_after_seconds`, plus `plan_type`.
//!
//! Fallback, no network: the limits Codex itself wrote on its last turn into
//! `sessions/YYYY/MM/DD/rollout-*.jsonl` (`payload.type == "token_count"`,
//! `rate_limits.{primary,secondary}` with `window_minutes`, `resets_at`).

use crate::limits_strip::{
    epoch_ms, http, json_or_error, net_err, now_ms, num, LimitWindow, ReadError, Reading, Secret,
    UsageProvider,
};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";
const TAIL_BYTES: u64 = 256 * 1024;

pub struct Codex;

pub fn codex_home() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("CODEX_HOME") {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    crate::limits_strip::home().map(|h| h.join(".codex"))
}

/// Codex names its windows only by length, and "5h limit" says more than
/// "primary". The primary window is not always five hours.
pub fn label_for_minutes(minutes: Option<f64>, fallback: &str) -> String {
    let Some(m) = minutes.filter(|m| *m > 0.0) else {
        return fallback.to_string();
    };
    let hours = m / 60.0;
    if (hours - 5.0).abs() < 0.5 {
        "5h limit".into()
    } else if (hours - 168.0).abs() < 12.0 {
        "Weekly limit".into()
    } else if (hours / 24.0 - 30.0).abs() <= 2.0 {
        "Monthly limit".into()
    } else if hours < 48.0 {
        format!("{}h limit", hours.round() as i64)
    } else {
        format!("{}d limit", (hours / 24.0).round() as i64)
    }
}

fn window_from(
    w: &serde_json::Value,
    id: &str,
    minutes: Option<f64>,
    now: i64,
    group: Option<&str>,
) -> Option<LimitWindow> {
    let pct = num(w.get("used_percent"))?;
    let resets = num(w.get("reset_at"))
        .or_else(|| num(w.get("resets_at")))
        .map(epoch_ms)
        .or_else(|| {
            num(w.get("reset_after_seconds"))
                .or_else(|| num(w.get("resets_in_seconds")))
                .map(|s| now + (s * 1000.0) as i64)
        });
    let mut out = LimitWindow::percent(id, &label_for_minutes(minutes, id), pct).resets(resets);
    out.span_ms = minutes.map(|m| (m * 60_000.0) as i64);
    out.group = group.map(str::to_string);
    Some(out)
}

pub fn parse_usage(v: &serde_json::Value, now: i64) -> Vec<LimitWindow> {
    let mut out = Vec::new();
    let mut pair = |rl: Option<&serde_json::Value>, ids: [&str; 2], group: Option<&str>| {
        let Some(rl) = rl.filter(|x| x.is_object()) else {
            return;
        };
        for (id, key) in ids.iter().zip(["primary_window", "secondary_window"]) {
            if let Some(w) = rl.get(key).filter(|w| w.is_object()) {
                let minutes = num(w.get("limit_window_seconds")).map(|s| s / 60.0);
                if let Some(win) = window_from(w, id, minutes, now, group) {
                    out.push(win);
                }
            }
        }
    };
    pair(v.get("rate_limit"), ["primary", "secondary"], None);
    if let Some(extras) = v.get("additional_rate_limits").and_then(|x| x.as_array()) {
        for extra in extras {
            let name = ["limit_name", "metered_feature"]
                .iter()
                .find_map(|k| extra.get(*k).and_then(|x| x.as_str()))
                .unwrap_or("");
            if name.to_lowercase().contains("spark") {
                pair(
                    extra.get("rate_limit"),
                    ["spark", "spark-secondary"],
                    Some("Spark"),
                );
            }
        }
    }
    pair(
        v.get("code_review_rate_limit"),
        ["code-review", "code-review-secondary"],
        Some("Code review"),
    );
    let mut seen = std::collections::HashSet::new();
    out.retain(|w: &LimitWindow| seen.insert(w.id.clone()));
    out
}

/// The last `token_count` line of a rollout log: `(windows, plan, line ts ms)`.
pub fn parse_rollout_tail(text: &str, now: i64) -> Option<(Vec<LimitWindow>, Option<String>, i64)> {
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(rl) = v.pointer("/payload/rate_limits").filter(|x| x.is_object()) else {
            continue;
        };
        let mut windows = Vec::new();
        for id in ["primary", "secondary"] {
            if let Some(w) = rl.get(id).filter(|w| w.is_object()) {
                if let Some(win) = window_from(w, id, num(w.get("window_minutes")), now, None) {
                    windows.push(win);
                }
            }
        }
        if windows.is_empty() {
            continue;
        }
        let plan = rl
            .get("plan_type")
            .and_then(|x| x.as_str())
            .map(str::to_string);
        let ts = crate::limits_strip::iso_ms(v.get("timestamp")).unwrap_or(0);
        return Some((windows, plan, ts));
    }
    None
}

fn newest_rollout(root: &Path) -> Option<PathBuf> {
    // sessions/YYYY/MM/DD: walk each level newest-first by name.
    fn newest_dirs(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        v.sort();
        v.reverse();
        v
    }
    for y in newest_dirs(root) {
        for m in newest_dirs(&y) {
            for d in newest_dirs(&m) {
                let best = std::fs::read_dir(&d)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|e| {
                        let n = e.file_name();
                        let n = n.to_string_lossy();
                        n.starts_with("rollout-") && n.ends_with(".jsonl")
                    })
                    .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
                    .max_by_key(|(t, _)| *t);
                if let Some((_, p)) = best {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn read_tail(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn from_rollout() -> Option<Reading> {
    let root = codex_home()?.join("sessions");
    let now = now_ms();
    let (windows, plan, ts) = parse_rollout_tail(&read_tail(&newest_rollout(&root)?)?, now)?;
    let age_min = ((now - ts).max(0)) / 60_000;
    Some(Reading {
        windows,
        plan,
        note: (age_min > 5).then(|| "as of the last Codex run".to_string()),
        ..Default::default()
    })
}

#[derive(Debug, PartialEq)]
pub struct Credential {
    access: Secret,
    account: String,
}

pub fn parse_credential(v: &serde_json::Value) -> Option<Credential> {
    let tokens = v.get("tokens")?;
    let access = Secret::new(tokens.get("access_token")?.as_str()?)?;
    let account = tokens.get("account_id")?.as_str()?.trim().to_string();
    (!account.is_empty()).then_some(Credential { access, account })
}

fn credential() -> Option<Credential> {
    parse_credential(&crate::limits_strip::read_json(
        &codex_home()?.join("auth.json"),
    )?)
}

#[async_trait::async_trait]
impl UsageProvider for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn label(&self) -> &'static str {
        "Codex"
    }

    async fn detect(&self) -> bool {
        codex_home()
            .map(|h| h.join("auth.json").is_file() || h.join("sessions").is_dir())
            .unwrap_or(false)
    }

    async fn read(&self) -> Result<Reading, ReadError> {
        let Some(cred) = credential() else {
            return tokio::task::spawn_blocking(from_rollout)
                .await
                .ok()
                .flatten()
                .ok_or_else(|| ReadError::NeedsAuth("no ChatGPT sign-in in Codex".into()));
        };
        let live = async {
            let resp = http()
                .get(ENDPOINT)
                .bearer_auth(cred.access.expose())
                .header("ChatGPT-Account-Id", &cred.account)
                .header("Accept", "application/json")
                .header("Cache-Control", "no-cache, no-store")
                .send()
                .await
                .map_err(net_err)?;
            json_or_error(resp).await
        }
        .await;
        match live {
            Ok(v) => {
                let windows = parse_usage(&v, now_ms());
                Ok(Reading {
                    note: windows
                        .is_empty()
                        .then(|| "nothing metered on this account".to_string()),
                    windows,
                    plan: v
                        .get("plan_type")
                        .and_then(|x| x.as_str())
                        .map(str::to_string),
                    ..Default::default()
                })
            }
            Err(e) => match tokio::task::spawn_blocking(from_rollout).await {
                Ok(Some(r)) => Ok(r),
                _ => Err(e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_live_reply_names_windows_by_their_length() {
        let v = serde_json::json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {"used_percent": 12.0, "limit_window_seconds": 18000, "reset_at": 1790585719},
                "secondary_window": {"used_percent": 40, "limit_window_seconds": 604800, "reset_after_seconds": 60}
            },
            "additional_rate_limits": [
                {"limit_name": "GPT-5-Spark", "rate_limit": {"primary_window": {"used_percent": 3, "limit_window_seconds": 18000}}},
                "junk"
            ],
            "code_review_rate_limit": null
        });
        let ws = parse_usage(&v, 1_000);
        assert_eq!(ws.len(), 3);
        assert_eq!(ws[0].label, "5h limit");
        assert_eq!(ws[0].resets_at, Some(1_790_585_719_000));
        assert_eq!(ws[1].label, "Weekly limit");
        assert_eq!(ws[1].resets_at, Some(61_000));
        assert_eq!(ws[2].group.as_deref(), Some("Spark"));
    }

    #[test]
    fn a_free_plan_with_a_monthly_primary_is_labelled_monthly() {
        assert_eq!(
            label_for_minutes(Some(43_200.0), "primary"),
            "Monthly limit"
        );
        assert_eq!(label_for_minutes(None, "primary"), "primary");
    }

    #[test]
    fn the_rollout_fallback_reads_the_last_token_count() {
        let text = concat!(
            r#"{"timestamp":"2026-09-01T10:00:00Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":1.0,"window_minutes":300,"resets_at":1790585719},"secondary":null,"plan_type":"free"}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T10:05:00Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":9.5,"window_minutes":300,"resets_at":1790585719},"secondary":{"used_percent":50,"window_minutes":10080,"resets_in_seconds":10},"plan_type":"plus"}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T10:06:00Z","type":"event_msg","payload":{"type":"agent_message"}}"#,
            "\n{broken"
        );
        let (ws, plan, ts) = parse_rollout_tail(text, 5_000).unwrap();
        assert_eq!(plan.as_deref(), Some("plus"));
        assert!((ws[0].used.unwrap() - 0.095).abs() < 1e-6);
        assert_eq!(ws[1].resets_at, Some(15_000));
        assert_eq!(ts, 1_788_257_100_000);
    }

    #[test]
    fn the_token_never_shows_in_debug_output() {
        let v = serde_json::json!({"tokens":{"access_token":"eyJ-SECRETVALUE","account_id":"acc"}});
        let c = parse_credential(&v).unwrap();
        assert_eq!(c.access.expose(), "eyJ-SECRETVALUE");
        assert!(!format!("{c:?}").contains("SECRETVALUE"));
        assert_eq!(
            parse_credential(&serde_json::json!({"OPENAI_API_KEY":"k"})),
            None
        );
    }
}
