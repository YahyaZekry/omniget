//! OpenCode (Go plan).
//!
//! Credential (read only): `auth.json` under OpenCode's data folder, which is
//! `$XDG_DATA_HOME/opencode` or `~/.local/share/opencode` on every OS (the
//! tool resolves XDG paths the same way on Windows and macOS). Root key
//! `opencode-go`, usually `{"type":"api","key":"..."}`.
//!
//! Endpoint: `GET https://opencode.ai/zen/go/v1/usage`. `percent` is 0-100
//! used; 403 means the key has no Go subscription, which is not an error.

use crate::limits_strip::{
    http, iso_ms, json_or_error, net_err, num, LimitWindow, ReadError, Reading, Secret,
    UsageProvider, DAY_MS, HOUR_MS,
};
use std::path::PathBuf;

const ENDPOINT: &str = "https://opencode.ai/zen/go/v1/usage";
pub const KEY_FIELDS: [&str; 7] = [
    "key",
    "apiKey",
    "api_key",
    "token",
    "accessToken",
    "access",
    "auth_token",
];

pub struct OpenCode;

pub fn auth_path() -> Option<PathBuf> {
    super::xdg_data_home().map(|d| d.join("opencode").join("auth.json"))
}

pub fn key_from(v: &serde_json::Value, id: &str) -> Option<Secret> {
    Secret::new(&super::string_or_field(v.get(id)?, &KEY_FIELDS)?)
}

pub fn parse_usage(v: &serde_json::Value) -> Vec<LimitWindow> {
    let mut out = Vec::new();
    for (id, label, span) in [
        ("rolling", "5h limit", 5 * HOUR_MS),
        ("weekly", "Weekly limit", 7 * DAY_MS),
        ("monthly", "Monthly limit", 30 * DAY_MS),
    ] {
        let Some(w) = v.pointer(&format!("/usage/{id}")) else {
            continue;
        };
        if let Some(pct) = num(w.get("percent")) {
            out.push(
                LimitWindow::percent(id, label, pct)
                    .resets(iso_ms(w.get("resetsAt")))
                    .span(span),
            );
        }
    }
    out
}

#[async_trait::async_trait]
impl UsageProvider for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }
    fn label(&self) -> &'static str {
        "OpenCode"
    }

    async fn detect(&self) -> bool {
        // Existence only: the file is not opened until the reader is on.
        auth_path().map(|p| p.is_file()).unwrap_or(false)
    }

    async fn read(&self) -> Result<Reading, ReadError> {
        let key = auth_path()
            .as_deref()
            .and_then(crate::limits_strip::read_json)
            .and_then(|v| key_from(&v, "opencode-go"))
            .ok_or_else(|| ReadError::NeedsAuth("no OpenCode Go key".into()))?;
        let resp = http()
            .get(ENDPOINT)
            .bearer_auth(key.expose())
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(net_err)?;
        if resp.status().as_u16() == 403 {
            return Ok(Reading {
                note: Some("no OpenCode Go subscription on this key".into()),
                ..Default::default()
            });
        }
        Ok(Reading {
            windows: parse_usage(&json_or_error(resp).await?),
            plan: Some("Go".into()),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_windows_with_used_percentages() {
        let v = serde_json::json!({"usage":{
            "rolling":{"status":"ok","percent":12,"resetsAt":"2026-09-06T12:31:06.611Z"},
            "weekly":{"status":"ok","percent":40.5,"resetsAt":"2026-09-10T00:00:00Z"},
            "monthly":{"status":"ok"}}});
        let ws = parse_usage(&v);
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].label, "5h limit");
        assert!((ws[1].used.unwrap() - 0.405).abs() < 1e-6);
    }

    #[test]
    fn the_key_is_found_as_a_string_or_inside_an_object_and_never_printed() {
        let v = serde_json::json!({"opencode-go":{"type":"api","key":"k-SECRETVALUE"},"plain":"z"});
        let k = key_from(&v, "opencode-go").unwrap();
        assert_eq!(k.expose(), "k-SECRETVALUE");
        assert!(!format!("{k:?}").contains("SECRETVALUE"));
        assert_eq!(key_from(&v, "plain").unwrap().expose(), "z");
        assert_eq!(key_from(&v, "nope"), None);
    }
}
