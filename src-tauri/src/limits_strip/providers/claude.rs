//! Claude Code.
//!
//! Credential (read only, never refreshed, never rewritten):
//!   - Linux and Windows: `<config>/.credentials.json`, where `<config>` is
//!     `$CLAUDE_CONFIG_DIR` or `~/.claude` (`%USERPROFILE%\.claude`).
//!   - macOS: the same file when it exists, otherwise the login keychain item
//!     `Claude Code-credentials` (account = the OS user). It is asked through
//!     `/usr/bin/security`, the tool Claude Code itself files the item with, so
//!     no keychain prompt is raised for OmniGet.
//!   - Shape: `{"claudeAiOauth":{"accessToken","expiresAt"(epoch ms),"subscriptionType"}}`.
//!
//! Endpoint: `GET https://api.anthropic.com/api/oauth/usage` with
//! `Authorization: Bearer` and `anthropic-beta: oauth-2025-04-20`, the same one
//! Claude Code's own `/usage` asks. An expired token is never sent: the
//! endpoint answers it with a long 429, not a 401.

use crate::limits_strip::{
    http, iso_ms, json_or_error, net_err, now_ms, LimitWindow, ReadError, Reading, Secret,
    UsageProvider, DAY_MS, HOUR_MS,
};
use std::path::PathBuf;

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";

pub struct Claude;

pub fn config_dir() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    crate::limits_strip::home().map(|h| h.join(".claude"))
}

#[derive(Debug, PartialEq)]
pub struct Credential {
    token: Secret,
    pub expires_at: Option<i64>,
    pub plan: Option<String>,
}

pub fn parse_credential(v: &serde_json::Value) -> Option<Credential> {
    let oauth = v.get("claudeAiOauth").unwrap_or(v);
    // An emptied credential is how the owner signs out.
    let token = Secret::new(oauth.get("accessToken")?.as_str()?)?;
    Some(Credential {
        token,
        expires_at: oauth
            .get("expiresAt")
            .and_then(|x| x.as_f64())
            .map(|ms| ms as i64)
            .filter(|ms| *ms > 0),
        plan: oauth
            .get("subscriptionType")
            .and_then(|x| x.as_str())
            .map(str::to_string),
    })
}

#[cfg(target_os = "macos")]
async fn keychain_credential() -> Option<Credential> {
    let user = std::env::var("USER").ok()?;
    let (ok, out, _) = super::run_cli(
        std::path::Path::new("/usr/bin/security"),
        &[
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-a",
            &user,
            "-w",
        ],
        &[],
        std::time::Duration::from_secs(5),
    )
    .await?;
    if !ok {
        return None;
    }
    parse_credential(&serde_json::from_str(out.trim()).ok()?)
}

#[cfg(not(target_os = "macos"))]
async fn keychain_credential() -> Option<Credential> {
    None
}

async fn credential() -> Option<Credential> {
    let file = config_dir().map(|d| d.join(".credentials.json"));
    if let Some(c) = file
        .as_deref()
        .and_then(crate::limits_strip::read_json)
        .as_ref()
        .and_then(parse_credential)
    {
        return Some(c);
    }
    keychain_credential().await
}

fn label_for(kind: &str, scope: Option<&str>) -> String {
    match kind {
        "session" | "five_hour" => "Current session".into(),
        "weekly_all" | "seven_day" | "weekly" => "Weekly (all models)".into(),
        "weekly_opus" | "seven_day_opus" => "Weekly (Opus)".into(),
        "weekly_sonnet" | "seven_day_sonnet" => "Weekly (Sonnet)".into(),
        "weekly_scoped" | "scoped" => format!("Weekly ({})", scope.unwrap_or("scoped")),
        other => {
            let s = other.trim_start_matches("weekly_").replace('_', " ");
            let mut c = s.chars();
            match c.next() {
                Some(f) => format!("Weekly ({}{})", f.to_uppercase(), c.as_str()),
                None => "Limit".into(),
            }
        }
    }
}

fn canonical(kind: &str) -> &str {
    match kind {
        "five_hour" => "session",
        "seven_day" | "weekly" => "weekly_all",
        "seven_day_opus" => "weekly_opus",
        "seven_day_sonnet" => "weekly_sonnet",
        k => k,
    }
}

fn span_of(id: &str) -> i64 {
    if id == "session" {
        5 * HOUR_MS
    } else {
        7 * DAY_MS
    }
}

/// `limits[]` is the forward-compatible shape; `five_hour` / `seven_day` are
/// merged in because a window that just rolled over vanishes from `limits`.
pub fn parse_usage(v: &serde_json::Value) -> Vec<LimitWindow> {
    let mut out: Vec<LimitWindow> = Vec::new();
    if let Some(arr) = v.get("limits").and_then(|x| x.as_array()) {
        for l in arr {
            let (Some(kind), Some(pct)) = (
                l.get("kind").and_then(|x| x.as_str()),
                l.get("percent").and_then(|x| x.as_f64()),
            ) else {
                continue;
            };
            let Some(resets) = iso_ms(l.get("resets_at")) else {
                continue;
            };
            let id = canonical(kind);
            if out.iter().any(|w| w.id == id) {
                continue;
            }
            let scope = l
                .pointer("/scope/model/display_name")
                .and_then(|x| x.as_str());
            out.push(
                LimitWindow::percent(id, &label_for(kind, scope), pct)
                    .resets(Some(resets))
                    .span(span_of(id)),
            );
        }
    }
    for field in [
        "five_hour",
        "seven_day",
        "seven_day_opus",
        "seven_day_sonnet",
    ] {
        let Some(w) = v.get(field).filter(|w| w.is_object()) else {
            continue;
        };
        let Some(u) = w.get("utilization").and_then(|x| x.as_f64()) else {
            continue;
        };
        let id = canonical(field);
        if out.iter().any(|x| x.id == id) {
            continue;
        }
        out.push(
            LimitWindow::percent(id, &label_for(field, None), u)
                .resets(iso_ms(w.get("resets_at")))
                .span(span_of(id)),
        );
    }
    out.sort_by_key(|w| match w.id.as_str() {
        "session" => (0, String::new()),
        "weekly_all" => (1, String::new()),
        other => (2, other.to_string()),
    });
    out
}

#[async_trait::async_trait]
impl UsageProvider for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }
    fn label(&self) -> &'static str {
        "Claude Code"
    }
    fn beta(&self) -> bool {
        false
    }

    async fn detect(&self) -> bool {
        config_dir().map(|d| d.is_dir()).unwrap_or(false)
    }

    async fn read(&self) -> Result<Reading, ReadError> {
        let Some(cred) = credential().await else {
            return Err(ReadError::NeedsAuth("no Claude Code login found".into()));
        };
        if cred.expires_at.map(|t| t <= now_ms()).unwrap_or(false) {
            return Err(ReadError::NeedsAuth(
                "login expired: run claude once to renew it".into(),
            ));
        }
        let resp = http()
            .get(ENDPOINT)
            .bearer_auth(cred.token.expose())
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .await
            .map_err(net_err)?;
        let v = json_or_error(resp).await?;
        let windows = parse_usage(&v);
        Ok(Reading {
            note: windows
                .is_empty()
                .then(|| "nothing metered on this account".to_string()),
            windows,
            plan: cred.plan,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_and_named_windows_merge_without_twins() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"five_hour":{"utilization":30.0,"resets_at":"2099-01-01T05:00:00.000000+00:00"},
                "seven_day":{"utilization":74.0,"resets_at":"2099-01-05T00:00:00.000000+00:00"},
                "limits":[{"kind":"weekly_all","percent":74,"resets_at":"2099-01-05T00:00:00Z"},
                          {"kind":"weekly_scoped","percent":55,"resets_at":"2099-01-05T00:00:00Z",
                           "scope":{"model":{"display_name":"Opus"}}},
                          {"kind":"no_reset","percent":10}]}"#,
        )
        .unwrap();
        let ws = parse_usage(&v);
        let ids: Vec<_> = ws.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["session", "weekly_all", "weekly_scoped"]);
        assert!((ws[0].used.unwrap() - 0.30).abs() < 1e-6);
        assert_eq!(ws[0].span_ms, Some(5 * HOUR_MS));
        assert_eq!(ws[2].label, "Weekly (Opus)");
    }

    #[test]
    fn an_emptied_credential_is_signed_out() {
        let v = serde_json::json!({"claudeAiOauth":{"accessToken":"","expiresAt":0}});
        assert_eq!(parse_credential(&v), None);
        let v = serde_json::json!({"claudeAiOauth":{"accessToken":"t","expiresAt":1.7e12,"subscriptionType":"max"}});
        let c = parse_credential(&v).unwrap();
        assert_eq!(c.token.expose(), "t");
        assert_eq!(c.plan.as_deref(), Some("max"));
        assert_eq!(c.expires_at, Some(1_700_000_000_000));
    }

    #[test]
    fn the_token_never_shows_in_debug_output() {
        let v = serde_json::json!({"claudeAiOauth":{"accessToken":"sk-ant-oat01-SECRETVALUE","expiresAt":1.7e12}});
        let c = parse_credential(&v).unwrap();
        assert!(!format!("{c:?}").contains("SECRETVALUE"));
    }
}
