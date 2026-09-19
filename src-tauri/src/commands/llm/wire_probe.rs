//! `llm_*` commands: wire_probe. Owned by f2-wire-probe.
//!
//! Two commands, both driven by a click:
//! - `llm_wire_probe_run` — estimates the round (`estimate_only: true`) or runs
//!   it and keeps the evidence;
//! - `llm_wire_probe_last` — the last round, so reopening the Observatory does
//!   not cost a request.
//!
//! The last round lives in this module (a `Mutex` behind a `OnceLock`), not in
//! `AppState`, because `LlmManager` belongs to f2-llm-commands. It is memory
//! only: nothing is written to disk and nothing survives a restart.
//!
//! Providers come from `LlmManager::provider_for` (f2-llm-commands), which
//! builds the client from the key vault on first use and caches it. An id with
//! no usable key answers `None`, and that becomes `ERR_LLM_MODEL` — the probe
//! never invents a client. `ProviderId("fake")` always resolves, so the panel
//! can be exercised without spending a cent.
//!
//! No `attach_app` here on purpose: the probe sends one plain message and grants
//! no tools, so it never touches the MCP executor that needs the `AppHandle`.

use std::sync::{Arc, Mutex, OnceLock};

use omniget_core::core::llm::error::ERR_LLM_MODEL;
use omniget_core::core::llm::providers::Provider;
use omniget_core::core::llm::types::{ModelRef, ProviderId};
use omniget_core::core::llm::wire_probe::{
    self, ProbeCase, ProbeRound, MAX_OUTPUT_TOKENS, MAX_PROBE_REQUESTS,
};
use serde_json::Value;
use tauri::State;

use crate::llm_manager::LlmManager;
use crate::AppState;

fn last_round() -> &'static Mutex<Option<ProbeRound>> {
    static LAST: OnceLock<Mutex<Option<ProbeRound>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Picks the cases by name; `None` means the standard round.
fn cases_for(names: Option<Vec<String>>) -> Vec<ProbeCase> {
    let all: Vec<ProbeCase> = wire_probe::default_cases()
        .into_iter()
        .chain(std::iter::once(wire_probe::reasoning_case()))
        .collect();
    match names {
        None => wire_probe::default_cases(),
        Some(wanted) => all
            .into_iter()
            .filter(|c| wanted.iter().any(|w| w == c.param))
            .collect(),
    }
}

/// The client for a provider id, or `ERR_LLM_MODEL` with the reason. The
/// manager owns the building; `probe_provider_for` is the same client with a
/// `WireCapture` attached, which is what turns a body on the wire into a real
/// verdict instead of `Unsupported`. This only turns `None` into an error the
/// UI can map.
fn resolve_provider(llm: &LlmManager, provider: &str) -> Result<Arc<dyn Provider>, String> {
    llm.probe_provider_for(&ProviderId::new(provider))
        .ok_or_else(|| {
            format!(
                "{ERR_LLM_MODEL}: no usable client for `{provider}` (no key, or unknown provider)"
            )
        })
}

/// The whole command, minus the Tauri wrapper, so a test can drive it with a
/// plain `LlmManager` and no `AppHandle`.
async fn run_round(
    llm: &LlmManager,
    provider: String,
    model: String,
    cases: Option<Vec<String>>,
    estimate_only: Option<bool>,
) -> Result<Value, String> {
    let cases = cases_for(cases);
    if cases.is_empty() {
        return Err(format!("{ERR_LLM_MODEL}: no probe case selected"));
    }

    if estimate_only.unwrap_or(false) {
        let est = wire_probe::estimate(&cases, None);
        return serde_json::to_value(est).map_err(|e| e.to_string());
    }

    let client = resolve_provider(llm, &provider)?;
    let target = ModelRef {
        provider: ProviderId::new(provider),
        model,
    };
    let started_at_ms = now_ms();
    let results = wire_probe::run(client, &target, &cases).await;
    let round = ProbeRound::new(&target, started_at_ms, results);
    let value = serde_json::to_value(&round).map_err(|e| e.to_string())?;
    if let Ok(mut slot) = last_round().lock() {
        *slot = Some(round);
    }
    Ok(value)
}

/// `estimate_only: true` costs nothing and answers what the round would cost.
/// Otherwise it runs at most 4 cases (8 requests, 200 output tokens each) and
/// stores the evidence.
#[tauri::command]
pub async fn llm_wire_probe_run(
    state: State<'_, AppState>,
    provider: String,
    model: String,
    cases: Option<Vec<String>>,
    estimate_only: Option<bool>,
) -> Result<Value, String> {
    run_round(&state.llm, provider, model, cases, estimate_only).await
}

/// `null` when no round has run in this session.
#[tauri::command]
pub async fn llm_wire_probe_last() -> Result<Value, String> {
    let round = last_round()
        .lock()
        .map_err(|_| "ERR_LLM_STATE: wire probe lock poisoned".to_string())?
        .clone();
    match round {
        Some(r) => serde_json::to_value(r).map_err(|e| e.to_string()),
        None => Ok(Value::Null),
    }
}

/// Limits the UI shows next to the button, kept in one place so the copy and
/// the enforcement cannot drift apart.
pub const PROBE_LIMITS: (usize, u32) = (MAX_PROBE_REQUESTS, MAX_OUTPUT_TOKENS);

#[cfg(test)]
mod tests {
    use super::*;

    /// A manager pointed at a directory of its own, so no test touches the
    /// owner's `<app_data>/llm`.
    fn manager() -> LlmManager {
        let llm = LlmManager::new();
        let root = std::env::temp_dir().join(format!(
            "omniget-wire-probe-{}-{}",
            std::process::id(),
            now_ms()
        ));
        llm.set_root(root);
        llm
    }

    #[test]
    fn default_round_is_the_four_standard_cases() {
        let c = cases_for(None);
        assert_eq!(c.len(), 4);
        assert_eq!(c[0].param, "temperature");
        assert_eq!(PROBE_LIMITS, (8, 200));
    }

    #[test]
    fn named_cases_can_include_the_optional_one() {
        let c = cases_for(Some(vec!["stop".into(), "reasoning_effort".into()]));
        assert_eq!(c.len(), 2);
        assert!(c.iter().any(|x| x.param == "reasoning_effort"));
        assert!(cases_for(Some(vec!["nope".into()])).is_empty());
    }

    #[test]
    fn the_manager_resolves_fake_and_refuses_the_unknown() {
        let llm = manager();
        // `Arc<dyn Provider>` is not Debug, so no `unwrap_err()` here.
        assert!(resolve_provider(&llm, "fake").is_ok());
        assert!(llm.provider_for(&ProviderId::new("fake")).is_some());

        let err = match resolve_provider(&llm, "not-a-provider") {
            Ok(_) => panic!("an unknown provider must not resolve"),
            Err(e) => e,
        };
        assert!(err.starts_with(ERR_LLM_MODEL), "{err}");
        assert!(llm
            .provider_for(&ProviderId::new("not-a-provider"))
            .is_none());
    }

    #[tokio::test]
    async fn estimate_only_spends_nothing() {
        let llm = manager();
        // The provider is never resolved on this path: an id with no key still
        // gets a price.
        let v = run_round(&llm, "openai".into(), "gpt-x".into(), None, Some(true))
            .await
            .unwrap();
        assert_eq!(v["requests"], serde_json::json!(8));
        assert_eq!(v["cost_usd"], Value::Null);
        assert_eq!(v["output_tokens"], serde_json::json!(1600));
    }

    #[tokio::test]
    async fn a_fake_round_is_stored_and_replayed() {
        let llm = manager();
        let v = run_round(
            &llm,
            "fake".into(),
            "fake-1".into(),
            Some(vec!["temperature".into()]),
            None,
        )
        .await
        .unwrap();
        assert_eq!(v["results"].as_array().unwrap().len(), 1);
        // The FakeProvider captures a body without any parameter, so the honest
        // verdict is `ignored`, never `proven`.
        assert_eq!(v["results"][0]["verdict"], serde_json::json!("ignored"));
        let last = llm_wire_probe_last().await.unwrap();
        assert_eq!(last["model"], serde_json::json!("fake-1"));
    }

    /// A real round through the command path (not just the core), against a
    /// local Ollama. Network, so `#[ignore]`. Needs `ollama serve` and the model
    /// in `OMNIGET_PROBE_MODEL` (default `qwen3:0.6b`):
    ///   cargo test -p omniget commands::llm::wire_probe::tests::ollama_round_via_command \
    ///     -- --ignored --nocapture
    ///
    /// It goes through `LlmManager::provider_for`, so it also proves the vault
    /// path builds a usable Ollama client without a key.
    ///
    /// What it asserts is the invariant, not a wish: a body on the wire means a
    /// real verdict, and no body means `Unsupported`. Today it lands on the
    /// second branch, because `OpenAiCompat`/`AnthropicProvider` only fill
    /// `WireCapture` when they are built with `.with_capture(...)` and
    /// `llm_manager::build_provider` does not do that yet (one line, owner
    /// f2-llm-commands; it is in this agent's handoff). The day it does, the
    /// same test starts proving `Proven`/`Sent` without a change here.
    #[tokio::test]
    #[ignore = "needs a local Ollama (network)"]
    async fn ollama_round_via_command() {
        let llm = manager();
        let model = std::env::var("OMNIGET_PROBE_MODEL").unwrap_or_else(|_| "qwen3:0.6b".into());
        let v = run_round(&llm, "ollama".into(), model, None, None)
            .await
            .expect("the round must not fail");
        let results = v["results"].as_array().expect("results");
        for r in results {
            println!(
                "{:<18} {:<12} differs={} detail={}",
                r["param"].as_str().unwrap_or(""),
                r["verdict"].as_str().unwrap_or(""),
                r["differs"],
                r["detail"].as_str().unwrap_or("")
            );
            println!(
                "  A {}",
                r["reply_a"].as_str().unwrap_or("").replace('\n', " / ")
            );
            println!(
                "  B {}",
                r["reply_b"].as_str().unwrap_or("").replace('\n', " / ")
            );
            println!("  sent_a {}", r["sent_a"]);
        }
        assert_eq!(results.len(), 4);

        // The round really reached the model: some turn came back with text.
        assert!(
            results
                .iter()
                .any(|r| !r["reply_a"].as_str().unwrap_or("").is_empty()
                    || !r["reply_b"].as_str().unwrap_or("").is_empty()),
            "no turn produced any text — Ollama did not answer: {v}"
        );

        for r in results {
            let captured = !r["sent_a"].is_null() && !r["sent_b"].is_null();
            if captured {
                assert_ne!(
                    r["verdict"], "unsupported",
                    "the body was captured, so the verdict must be a real one: {r}"
                );
            } else {
                assert_eq!(
                    r["verdict"], "unsupported",
                    "no body captured means no evidence, so the verdict must be Unsupported: {r}"
                );
                assert!(r["detail"]
                    .as_str()
                    .unwrap_or("")
                    .contains("no wire capture"));
            }
        }

        // And the round is the one the panel will read back.
        assert_eq!(llm_wire_probe_last().await.unwrap()["provider"], "ollama");
    }
}
