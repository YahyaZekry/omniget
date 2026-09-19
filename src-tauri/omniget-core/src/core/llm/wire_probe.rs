//! Wire probe: proves a generation parameter reached the provider and changed
//! the reply. Owned by f2-wire-probe.
//!
//! HTTP 200 is not evidence. For each parameter the probe runs the same prompt
//! twice with two different values and then looks at three things:
//!
//! 1. the body the provider actually put on the wire (`WireCapture`, already
//!    redacted by the provider),
//! 2. whether the parameter is in that body with the value we asked for,
//! 3. whether the two replies differ.
//!
//! From those three facts it gives a verdict:
//!
//! | body has it | replies differ | verdict       |
//! |-------------|----------------|---------------|
//! | yes         | yes            | `Proven`      |
//! | yes         | no             | `Sent`        |
//! | no          | —              | `Ignored`     |
//! | turn failed / no capture | — | `Unsupported` |
//!
//! `Ignored` means our own client dropped the parameter before the request left
//! the machine; `Unsupported` means no evidence could be obtained at all (the
//! provider refused the request, or it exposes no wire capture).
//!
//! Budget (plan: one standard round ≤ 8 requests, ≤ 200 output tokens each) is
//! enforced here, not by the caller: cases past [`MAX_PROBE_REQUESTS`] come back
//! as `Unsupported` with `ERR_LLM_BUDGET` in `detail`, never as extra traffic.
//! Nothing in this module runs on its own: `run` is only reached by a click.

use std::sync::Arc;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::error::{LlmError, ERR_LLM_BUDGET, ERR_LLM_PARSE};
use super::providers::Provider;
use super::types::{GenParams, Message, ModelRef, ProviderId, Role, TurnEvent, TurnRequest};

/// Hard ceiling on requests for one round (4 cases × 2 runs).
pub const MAX_PROBE_REQUESTS: usize = 8;
/// Output ceiling per request. Every probe prompt is designed to fit in it.
pub const MAX_OUTPUT_TOKENS: u32 = 200;
/// Fixed seed, so two runs of the same prompt only differ because of the
/// parameter under test. Only sent to providers that accept it.
pub const PROBE_SEED: u64 = 7;

/// One parameter under test: the same prompt run with value `a` and value `b`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeCase {
    pub param: &'static str,
    pub a: Value,
    pub b: Value,
    pub prompt: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// In the body with the value we asked for, and the reply changed.
    Proven,
    /// In the body with the value we asked for, but the reply did not change.
    Sent,
    /// Our client did not put it on the wire (or rewrote the value).
    Ignored,
    /// No evidence possible: the turn failed, or the provider has no capture.
    Unsupported,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Proven => "proven",
            Verdict::Sent => "sent",
            Verdict::Ignored => "ignored",
            Verdict::Unsupported => "unsupported",
        }
    }
}

/// Evidence for one case. `sent_a`/`sent_b` are the whole redacted bodies, so
/// the UI can diff them; `Value::Null` means the provider captured nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub provider: String,
    pub model: String,
    pub param: String,
    pub sent_a: Value,
    pub sent_b: Value,
    pub reply_a: String,
    pub reply_b: String,
    pub differs: bool,
    pub cost_usd: f64,
    pub verdict: Verdict,
    /// Where the parameter was found in the body (JSON path) or why the verdict
    /// is not `Proven`. Stable `ERR_LLM_*` codes when it comes from an error.
    pub detail: String,
}

/// What a round will cost before anyone clicks. Shown next to the button.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProbeEstimate {
    pub cases: usize,
    pub requests: usize,
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// `None` when the model has no known price (local models cost nothing but
    /// are still reported as `Some(0.0)` by the caller that knows that).
    pub cost_usd: Option<f64>,
}

/// USD per million tokens, as the roster stores it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TokenPrice {
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
}

/// The standard round: exactly 8 requests.
pub fn default_cases() -> Vec<ProbeCase> {
    vec![
        ProbeCase {
            param: "temperature",
            a: Value::from(0.0),
            b: Value::from(1.5),
            prompt: "Write one short sentence about the sea.",
        },
        ProbeCase {
            param: "top_p",
            a: Value::from(1.0),
            b: Value::from(0.05),
            prompt: "Write one short sentence about the sea.",
        },
        ProbeCase {
            param: "max_tokens",
            a: Value::from(200),
            b: Value::from(8),
            prompt: "List the numbers from 1 to 40, separated by commas.",
        },
        ProbeCase {
            param: "stop",
            a: Value::Array(vec![]),
            b: Value::Array(vec![Value::from("5")]),
            prompt: "Count from 1 to 9. Only the digits, one per line, nothing else.",
        },
    ]
}

/// Optional extra case; it pushes the round past the standard budget on its own,
/// so the UI offers it as a separate run.
pub fn reasoning_case() -> ProbeCase {
    ProbeCase {
        param: "reasoning_effort",
        a: Value::from("low"),
        b: Value::from("high"),
        prompt: "How many r's are in strawberry? Answer with the number only.",
    }
}

pub fn estimate(cases: &[ProbeCase], price: Option<TokenPrice>) -> ProbeEstimate {
    let runs = cases.len().min(MAX_PROBE_REQUESTS / 2);
    let requests = runs * 2;
    // ~4 chars per token, plus a small envelope for the role/format overhead.
    let input_tokens: u32 = cases
        .iter()
        .take(runs)
        .map(|c| (c.prompt.len() as u32).div_ceil(4) + 8)
        .sum::<u32>()
        * 2;
    let output_tokens = MAX_OUTPUT_TOKENS * requests as u32;
    let cost_usd = price.map(|p| {
        (input_tokens as f64 * p.input_usd_per_mtok + output_tokens as f64 * p.output_usd_per_mtok)
            / 1_000_000.0
    });
    ProbeEstimate {
        cases: runs,
        requests,
        input_tokens,
        output_tokens,
        cost_usd,
    }
}

/// Providers whose bodies accept a `seed`, so temperature/top_p runs are
/// otherwise deterministic. This is provider configuration, not a guess about
/// the machine. Anything else gets no seed (an unknown key can be a 400).
fn accepts_seed(provider: &ProviderId) -> bool {
    matches!(
        provider.as_str(),
        "openai" | "openrouter" | "ollama" | "lmstudio" | "llamacpp" | "groq" | "fake"
    )
}

/// Body keys that carry each parameter across the providers we speak to.
/// `max_tokens` alone appears under four names (OpenAI's two, Anthropic's and
/// Ollama's), which is exactly why "it returned 200" proves nothing.
fn aliases(param: &str) -> &'static [&'static str] {
    match param {
        "temperature" => &["temperature"],
        "top_p" => &["top_p", "topP"],
        "max_tokens" => &[
            "max_tokens",
            "max_completion_tokens",
            "max_output_tokens",
            "num_predict",
        ],
        "stop" => &["stop", "stop_sequences", "stop_words"],
        "reasoning_effort" => &["reasoning_effort", "reasoning", "thinking"],
        _ => &[],
    }
}

/// Writes one parameter into `GenParams`. Unknown names go to `extra`, which is
/// how a provider-specific knob is probed without a new type.
pub fn apply_param(params: &mut GenParams, param: &str, value: &Value) -> Result<(), LlmError> {
    let bad = |want: &str| {
        Err(LlmError::new(
            ERR_LLM_PARSE,
            format!("probe case `{param}` needs {want}, got {value}"),
        ))
    };
    match param {
        "temperature" => match value.as_f64() {
            Some(v) => params.temperature = Some(v as f32),
            None => return bad("a number"),
        },
        "top_p" => match value.as_f64() {
            Some(v) => params.top_p = Some(v as f32),
            None => return bad("a number"),
        },
        "max_tokens" => match value.as_u64() {
            Some(v) => params.max_tokens = Some(v.min(MAX_OUTPUT_TOKENS as u64) as u32),
            None => return bad("a whole number"),
        },
        "reasoning_effort" => match value.as_str() {
            Some(v) => params.reasoning_effort = Some(v.to_string()),
            None => return bad("a string"),
        },
        "stop" => match value.as_array() {
            Some(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    match it.as_str() {
                        Some(s) => out.push(s.to_string()),
                        None => return bad("an array of strings"),
                    }
                }
                params.stop = out;
            }
            None => return bad("an array of strings"),
        },
        other => {
            params.extra.insert(other.to_string(), value.clone());
        }
    }
    Ok(())
}

/// Depth-first search for any of `keys`, returning the JSON path where it was
/// found. Nested on purpose: Ollama hides the knobs under `options`, and more
/// than one gateway hides them under `extra_body`.
fn find_key<'a>(
    body: &'a Value,
    keys: &[&str],
    path: &str,
    depth: u8,
) -> Option<(String, &'a Value)> {
    if depth > 6 {
        return None;
    }
    match body {
        Value::Object(map) => {
            for k in keys {
                if let Some(v) = map.get(*k) {
                    if !v.is_null() {
                        return Some((join_path(path, k), v));
                    }
                }
            }
            for (k, v) in map {
                if let Some(found) = find_key(v, keys, &join_path(path, k), depth + 1) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                if let Some(found) = find_key(v, keys, &format!("{path}[{i}]"), depth + 1) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn join_path(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

/// Is the value on the wire the one we asked for? Numbers are compared with a
/// tolerance (1.5 can travel as 1.5000001), and a scalar also counts as matched
/// when it sits inside the object or array the provider built around it
/// (`reasoning_effort: "high"` → `{"reasoning": {"effort": "high"}}`).
pub fn values_match(sent: &Value, asked: &Value) -> bool {
    match (sent, asked) {
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-6 * y.abs().max(1.0),
            _ => a == b,
        },
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            a.iter().zip(b).all(|(x, y)| values_match(x, y))
        }
        (Value::String(_), Value::Array(b)) if b.len() == 1 => values_match(sent, &b[0]),
        (Value::Array(a), _) if !asked.is_array() => a.iter().any(|x| values_match(x, asked)),
        (Value::Object(map), _) if !asked.is_object() => {
            map.values().any(|v| values_match(v, asked))
        }
        _ => sent == asked,
    }
}

/// A value that asks for nothing (`null`, `[]`, `""`) may legitimately be left
/// out of the body; presence is only required on the other side of the pair.
fn requires_presence(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Array(a) => !a.is_empty(),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// Trailing whitespace and stray blank lines are formatting, not a difference.
fn normalize_reply(s: &str) -> String {
    s.lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

struct TurnOutcome {
    sent: Value,
    reply: String,
    cost_usd: f64,
    error: Option<LlmError>,
}

async fn one_turn(
    provider: &Arc<dyn Provider>,
    model: &ModelRef,
    case: &ProbeCase,
    value: &Value,
) -> TurnOutcome {
    let mut params = GenParams {
        max_tokens: Some(MAX_OUTPUT_TOKENS),
        ..GenParams::default()
    };
    if accepts_seed(&model.provider) {
        params
            .extra
            .insert("seed".to_string(), Value::from(PROBE_SEED));
    }
    if let Err(e) = apply_param(&mut params, case.param, value) {
        return TurnOutcome {
            sent: Value::Null,
            reply: String::new(),
            cost_usd: 0.0,
            error: Some(e),
        };
    }

    let req = TurnRequest {
        model: model.clone(),
        messages: vec![Message::text(Role::User, case.prompt)],
        tools: vec![],
        params,
        cancel: CancellationToken::new(),
        agent_id: None,
    };

    let mut reply = String::new();
    let mut cost_usd = 0.0;
    let mut error = None;
    match provider.turn(req).await {
        Ok(mut stream) => {
            while let Some(ev) = stream.next().await {
                match ev {
                    TurnEvent::TextDelta { text } => reply.push_str(&text),
                    TurnEvent::Usage { usage } => cost_usd += usage.cost_usd.unwrap_or(0.0),
                    TurnEvent::Error { error: e } => error = Some(e),
                    _ => {}
                }
            }
        }
        Err(e) => error = Some(e),
    }

    let sent = provider
        .wire_capture()
        .and_then(|c| c.snapshot().sent_body)
        .unwrap_or(Value::Null);

    TurnOutcome {
        sent,
        reply,
        cost_usd,
        error,
    }
}

fn judge(case: &ProbeCase, a: &TurnOutcome, b: &TurnOutcome, differs: bool) -> (Verdict, String) {
    if let Some(e) = a.error.as_ref().or(b.error.as_ref()) {
        return (Verdict::Unsupported, format!("{}: {}", e.code, e.message));
    }
    if a.sent.is_null() || b.sent.is_null() {
        return (
            Verdict::Unsupported,
            "provider exposes no wire capture".to_string(),
        );
    }

    let keys: Vec<&str> = {
        let named = aliases(case.param);
        if named.is_empty() {
            vec![case.param]
        } else {
            named.to_vec()
        }
    };

    let mut where_found = String::new();
    for (outcome, asked) in [(a, &case.a), (b, &case.b)] {
        match find_key(&outcome.sent, &keys, "", 0) {
            Some((path, got)) => {
                if !values_match(got, asked) {
                    if requires_presence(asked) {
                        return (
                            Verdict::Ignored,
                            format!("`{path}` carried {got}, not {asked}"),
                        );
                    }
                } else if where_found.is_empty() {
                    where_found = path;
                }
            }
            None => {
                if requires_presence(asked) {
                    return (
                        Verdict::Ignored,
                        format!("`{}` is not in the body sent", case.param),
                    );
                }
            }
        }
    }

    if where_found.is_empty() {
        where_found = case.param.to_string();
    }
    if differs {
        (Verdict::Proven, where_found)
    } else {
        (Verdict::Sent, format!("{where_found}; reply unchanged"))
    }
}

/// Runs the cases in order, two requests each, sequentially (a probe is never
/// worth a burst). Never panics and never returns early: a failing case is a
/// result with a verdict, not a lost round.
pub async fn run(
    provider: Arc<dyn Provider>,
    model: &ModelRef,
    cases: &[ProbeCase],
) -> Vec<ProbeResult> {
    let allowed = MAX_PROBE_REQUESTS / 2;
    let mut out = Vec::with_capacity(cases.len());
    for (idx, case) in cases.iter().enumerate() {
        if idx >= allowed {
            out.push(ProbeResult {
                provider: model.provider.as_str().to_string(),
                model: model.model.clone(),
                param: case.param.to_string(),
                sent_a: Value::Null,
                sent_b: Value::Null,
                reply_a: String::new(),
                reply_b: String::new(),
                differs: false,
                cost_usd: 0.0,
                verdict: Verdict::Unsupported,
                detail: format!("{ERR_LLM_BUDGET}: over {MAX_PROBE_REQUESTS} requests per round"),
            });
            continue;
        }

        let a = one_turn(&provider, model, case, &case.a).await;
        let b = one_turn(&provider, model, case, &case.b).await;
        let differs = normalize_reply(&a.reply) != normalize_reply(&b.reply);
        let (verdict, detail) = judge(case, &a, &b, differs);
        out.push(ProbeResult {
            provider: model.provider.as_str().to_string(),
            model: model.model.clone(),
            param: case.param.to_string(),
            sent_a: a.sent,
            sent_b: b.sent,
            reply_a: a.reply,
            reply_b: b.reply,
            differs,
            cost_usd: a.cost_usd + b.cost_usd,
            verdict,
            detail,
        });
    }
    out
}

/// A whole round, ready for the UI: the results plus what it cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeRound {
    pub provider: String,
    pub model: String,
    pub started_at_ms: u64,
    pub results: Vec<ProbeResult>,
    pub cost_usd: f64,
}

impl ProbeRound {
    pub fn new(model: &ModelRef, started_at_ms: u64, results: Vec<ProbeResult>) -> Self {
        let cost_usd = results.iter().map(|r| r.cost_usd).sum();
        Self {
            provider: model.provider.as_str().to_string(),
            model: model.model.clone(),
            started_at_ms,
            results,
            cost_usd,
        }
    }

    pub fn count(&self, verdict: Verdict) -> usize {
        self.results.iter().filter(|r| r.verdict == verdict).count()
    }
}

/// Convenience for a provider client writing its own capture: strips the keys
/// that must never reach the UI, whatever nesting they sit in.
pub fn redact(body: &Value) -> Value {
    const SECRET_KEYS: &[&str] = &[
        "authorization",
        "api_key",
        "api-key",
        "x-api-key",
        "key",
        "token",
        "access_token",
        "password",
        "cookie",
    ];
    match body {
        Value::Object(map) => {
            let mut out = Map::with_capacity(map.len());
            for (k, v) in map {
                if SECRET_KEYS.contains(&k.to_ascii_lowercase().as_str()) {
                    out.insert(k.clone(), Value::from("[redacted]"));
                } else {
                    out.insert(k.clone(), redact(v));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        _ => body.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::llm::providers::{fake::FakeProvider, WireCapture};
    use crate::core::llm::types::FinishReason;
    use async_trait::async_trait;
    use futures::stream::{self, BoxStream};
    use serde_json::json;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn model(provider: &str) -> ModelRef {
        ModelRef {
            provider: ProviderId::new(provider),
            model: "probe-model".into(),
        }
    }

    // ---------- pure functions ----------

    #[test]
    fn finds_a_nested_alias() {
        // Ollama shape: the knobs live under `options`, with its own names.
        let body =
            json!({ "model": "llama3", "options": { "num_predict": 64, "temperature": 0.2 } });
        let (path, v) = find_key(&body, aliases("max_tokens"), "", 0).unwrap();
        assert_eq!(path, "options.num_predict");
        assert_eq!(v, &json!(64));
    }

    #[test]
    fn missing_key_is_not_found() {
        let body = json!({ "model": "x", "messages": [] });
        assert!(find_key(&body, aliases("top_p"), "", 0).is_none());
        // An explicit null counts as absent, not as a value.
        let nulled = json!({ "top_p": null });
        assert!(find_key(&nulled, aliases("top_p"), "", 0).is_none());
    }

    #[test]
    fn values_match_tolerates_float_noise_and_wrappers() {
        assert!(values_match(&json!(1.5000000001), &json!(1.5)));
        assert!(!values_match(&json!(1.0), &json!(1.5)));
        assert!(values_match(&json!({ "effort": "high" }), &json!("high")));
        assert!(values_match(&json!(["5"]), &json!(["5"])));
        assert!(!values_match(&json!(["6"]), &json!(["5"])));
    }

    #[test]
    fn apply_param_writes_typed_fields_and_extras() {
        let mut p = GenParams::default();
        apply_param(&mut p, "temperature", &json!(1.5)).unwrap();
        apply_param(&mut p, "stop", &json!(["5"])).unwrap();
        apply_param(&mut p, "max_tokens", &json!(9999)).unwrap();
        apply_param(&mut p, "mirostat", &json!(2)).unwrap();
        assert_eq!(p.temperature, Some(1.5));
        assert_eq!(p.stop, vec!["5".to_string()]);
        assert_eq!(p.max_tokens, Some(MAX_OUTPUT_TOKENS)); // clamped to the budget
        assert_eq!(p.extra.get("mirostat"), Some(&json!(2)));
        let err = apply_param(&mut p, "temperature", &json!("hot")).unwrap_err();
        assert_eq!(err.code, ERR_LLM_PARSE);
    }

    #[test]
    fn redact_hides_secrets_at_any_depth() {
        let body = json!({ "headers": { "Authorization": "Bearer sk-live" }, "model": "x" });
        let out = redact(&body);
        assert_eq!(out["headers"]["Authorization"], json!("[redacted]"));
        assert_eq!(out["model"], json!("x"));
    }

    #[test]
    fn estimate_caps_at_the_round_budget() {
        let mut cases = default_cases();
        cases.push(reasoning_case());
        let e = estimate(
            &cases,
            Some(TokenPrice {
                input_usd_per_mtok: 1.0,
                output_usd_per_mtok: 2.0,
            }),
        );
        assert_eq!(e.cases, 4);
        assert_eq!(e.requests, MAX_PROBE_REQUESTS);
        assert_eq!(e.output_tokens, MAX_OUTPUT_TOKENS * 8);
        assert!(e.cost_usd.unwrap() > 0.0);
        assert!(estimate(&cases, None).cost_usd.is_none());
    }

    // ---------- mock HTTP server + a provider that really speaks to it ----------

    /// Minimal HTTP/1.1 echo server. `axum` is not a dependency of this crate
    /// (and a probe test is not worth one), so this is 40 lines of tokio:
    /// read the headers, read `Content-Length` bytes, answer, close.
    ///
    /// Behaviour, on purpose:
    /// - body carries `boom` → 400 (the provider surfaces an error → Unsupported)
    /// - `temperature >= 1.0` → "hot answer", otherwise "cold answer"
    /// - a non-empty `stop` truncates the reply at the first match
    /// - `top_p` is read and deliberately ignored (→ Sent)
    async fn spawn_mock() -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    let mut body_start = None;
                    let mut content_length = 0usize;
                    loop {
                        let n = match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => n,
                        };
                        buf.extend_from_slice(&chunk[..n]);
                        if body_start.is_none() {
                            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                                for line in head.lines() {
                                    if let Some(v) = line.strip_prefix("content-length:") {
                                        content_length = v.trim().parse().unwrap_or(0);
                                    }
                                }
                                body_start = Some(pos + 4);
                            }
                        }
                        if let Some(start) = body_start {
                            if buf.len() >= start + content_length {
                                break;
                            }
                        }
                    }
                    let start = body_start.unwrap_or(buf.len());
                    let body: Value = serde_json::from_slice(&buf[start..]).unwrap_or(Value::Null);

                    let (status, payload) = if body.get("boom").is_some() {
                        (400, json!({ "error": "unknown parameter `boom`" }))
                    } else {
                        let temp = body
                            .get("temperature")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        let mut reply = if temp >= 1.0 {
                            "hot answer".to_string()
                        } else {
                            "cold answer".to_string()
                        };
                        if let Some(stops) = body.get("stop").and_then(|v| v.as_array()) {
                            for s in stops.iter().filter_map(|s| s.as_str()) {
                                if let Some(at) = reply.find(s) {
                                    reply.truncate(at);
                                }
                            }
                        }
                        (200, json!({ "reply": reply }))
                    };
                    let text = payload.to_string();
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                        text.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        (addr, handle)
    }

    /// A real HTTP provider, as thin as it gets: it builds a body, sends it,
    /// records it in `WireCapture` and turns the answer into events. It drops
    /// `reasoning_effort` on the floor, which is exactly the `Ignored` case the
    /// probe has to catch.
    struct HttpProbeProvider {
        url: String,
        client: reqwest::Client,
        capture: Arc<WireCapture>,
    }

    impl HttpProbeProvider {
        fn new(addr: SocketAddr) -> Self {
            Self {
                url: format!("http://{addr}/v1/chat"),
                client: reqwest::Client::new(),
                capture: Arc::new(WireCapture::default()),
            }
        }
    }

    #[async_trait]
    impl Provider for HttpProbeProvider {
        async fn turn(&self, req: TurnRequest) -> Result<BoxStream<'static, TurnEvent>, LlmError> {
            let mut body = json!({ "model": req.model.model, "messages": [] });
            let map = body.as_object_mut().unwrap();
            if let Some(v) = req.params.temperature {
                map.insert("temperature".into(), json!(v));
            }
            if let Some(v) = req.params.top_p {
                map.insert("top_p".into(), json!(v));
            }
            if let Some(v) = req.params.max_tokens {
                map.insert("max_tokens".into(), json!(v));
            }
            if !req.params.stop.is_empty() {
                map.insert("stop".into(), json!(req.params.stop));
            }
            // reasoning_effort is intentionally NOT serialised.
            for (k, v) in &req.params.extra {
                map.insert(k.clone(), v.clone());
            }

            let resp = self
                .client
                .post(&self.url)
                .json(&body)
                .send()
                .await
                .map_err(|e| LlmError::new(crate::core::llm::error::ERR_LLM_NET, e.to_string()))?;
            let status = resp.status();
            let raw = resp.text().await.unwrap_or_default();
            self.capture.record(redact(&body), &raw);

            if !status.is_success() {
                return Err(LlmError::new(
                    crate::core::llm::error::ERR_LLM_MODEL,
                    format!("HTTP {status}: {raw}"),
                ));
            }
            let parsed: Value = serde_json::from_str(&raw)
                .map_err(|e| LlmError::new(ERR_LLM_PARSE, e.to_string()))?;
            let text = parsed
                .get("reply")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let events = vec![
                TurnEvent::TextDelta { text },
                TurnEvent::Finished {
                    reason: FinishReason::Stop,
                },
            ];
            Ok(stream::iter(events).boxed())
        }

        fn wire_capture(&self) -> Option<Arc<WireCapture>> {
            Some(self.capture.clone())
        }
    }

    #[tokio::test]
    async fn mock_round_proves_sent_ignored_and_unsupported() {
        let (addr, server) = spawn_mock().await;
        let provider: Arc<dyn Provider> = Arc::new(HttpProbeProvider::new(addr));
        let m = model("openai");

        let cases = vec![
            // reaches the body and changes the reply
            ProbeCase {
                param: "temperature",
                a: json!(0.0),
                b: json!(1.5),
                prompt: "say something",
            },
            // reaches the body, reply identical
            ProbeCase {
                param: "top_p",
                a: json!(1.0),
                b: json!(0.05),
                prompt: "say something",
            },
            // the client never serialises it
            ProbeCase {
                param: "reasoning_effort",
                a: json!("low"),
                b: json!("high"),
                prompt: "say something",
            },
            // the server refuses the request
            ProbeCase {
                param: "boom",
                a: json!(1),
                b: json!(2),
                prompt: "say something",
            },
        ];

        let out = run(provider, &m, &cases).await;
        assert_eq!(out.len(), 4);

        assert_eq!(out[0].verdict, Verdict::Proven, "{:?}", out[0]);
        assert!(out[0].differs);
        assert_eq!(out[0].reply_a, "cold answer");
        assert_eq!(out[0].reply_b, "hot answer");
        assert_eq!(out[0].sent_a["temperature"], json!(0.0));
        assert_eq!(out[0].sent_b["temperature"], json!(1.5));
        // the seed travels for a provider that accepts it
        assert_eq!(out[0].sent_a["seed"], json!(PROBE_SEED));

        assert_eq!(out[1].verdict, Verdict::Sent, "{:?}", out[1]);
        assert!(!out[1].differs);
        assert!(out[1].detail.contains("reply unchanged"));

        assert_eq!(out[2].verdict, Verdict::Ignored, "{:?}", out[2]);
        assert!(out[2].detail.contains("not in the body"));

        assert_eq!(out[3].verdict, Verdict::Unsupported, "{:?}", out[3]);
        assert!(out[3].detail.contains("ERR_LLM_MODEL"));

        server.abort();
    }

    #[tokio::test]
    async fn stop_is_proven_even_though_the_empty_side_is_absent() {
        let (addr, server) = spawn_mock().await;
        let provider: Arc<dyn Provider> = Arc::new(HttpProbeProvider::new(addr));
        let cases = vec![ProbeCase {
            param: "stop",
            a: json!([]),
            b: json!(["answer"]),
            prompt: "count",
        }];
        let out = run(provider, &model("openai"), &cases).await;
        assert_eq!(out[0].verdict, Verdict::Proven, "{:?}", out[0]);
        assert_eq!(out[0].reply_a, "cold answer");
        assert_eq!(out[0].reply_b, "cold ");
        assert!(out[0].sent_a.get("stop").is_none());
        server.abort();
    }

    #[tokio::test]
    async fn fake_provider_reports_ignored_not_proven() {
        // FakeProvider captures a body without any parameter: replies are equal
        // and nothing is on the wire, so the honest answer is Ignored.
        let provider: Arc<dyn Provider> = Arc::new(FakeProvider::text("same answer", 2));
        let out = run(provider, &model("fake"), &default_cases()).await;
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|r| r.verdict == Verdict::Ignored), "{out:?}");
        assert!(out.iter().all(|r| !r.differs));
    }

    #[tokio::test]
    async fn a_provider_without_capture_is_unsupported() {
        struct Blind;
        #[async_trait]
        impl Provider for Blind {
            async fn turn(
                &self,
                _req: TurnRequest,
            ) -> Result<BoxStream<'static, TurnEvent>, LlmError> {
                Ok(stream::iter(vec![TurnEvent::TextDelta { text: "x".into() }]).boxed())
            }
        }
        let out = run(Arc::new(Blind), &model("mystery"), &default_cases()[..1]).await;
        assert_eq!(out[0].verdict, Verdict::Unsupported);
        assert!(out[0].detail.contains("no wire capture"));
    }

    #[tokio::test]
    async fn the_fifth_case_is_refused_by_the_budget() {
        let mut cases = default_cases();
        cases.push(reasoning_case());
        let provider: Arc<dyn Provider> = Arc::new(FakeProvider::text("same", 1));
        let out = run(provider, &model("fake"), &cases).await;
        assert_eq!(out.len(), 5);
        assert_eq!(out[4].verdict, Verdict::Unsupported);
        assert!(out[4].detail.starts_with(ERR_LLM_BUDGET));
        assert_eq!(out[4].reply_a, "");
    }

    #[test]
    fn round_sums_the_cost_and_counts_verdicts() {
        let m = model("openai");
        let mk = |verdict, cost| ProbeResult {
            provider: "openai".into(),
            model: "probe-model".into(),
            param: "temperature".into(),
            sent_a: Value::Null,
            sent_b: Value::Null,
            reply_a: String::new(),
            reply_b: String::new(),
            differs: false,
            cost_usd: cost,
            verdict,
            detail: String::new(),
        };
        let round = ProbeRound::new(
            &m,
            1,
            vec![mk(Verdict::Proven, 0.01), mk(Verdict::Sent, 0.02)],
        );
        assert!((round.cost_usd - 0.03).abs() < 1e-9);
        assert_eq!(round.count(Verdict::Proven), 1);
        assert_eq!(round.count(Verdict::Ignored), 0);
    }

    /// Real round against a local Ollama. Needs `ollama serve` plus the model in
    /// `OMNIGET_PROBE_MODEL` (default `llama3.2:1b`). Network, so `#[ignore]`:
    ///   cargo test -p omniget-core --features desktop \
    ///     llm::wire_probe::tests::ollama_round -- --ignored --nocapture
    ///
    /// Kept here rather than in the provider crate because it is the only test
    /// that answers the product question: does *this* server honour the knobs?
    #[tokio::test]
    #[ignore = "needs a local Ollama (network)"]
    async fn ollama_round() {
        struct Ollama {
            client: reqwest::Client,
            capture: Arc<WireCapture>,
            model: String,
        }
        #[async_trait]
        impl Provider for Ollama {
            async fn turn(
                &self,
                req: TurnRequest,
            ) -> Result<BoxStream<'static, TurnEvent>, LlmError> {
                let mut options = serde_json::Map::new();
                if let Some(v) = req.params.temperature {
                    options.insert("temperature".into(), json!(v));
                }
                if let Some(v) = req.params.top_p {
                    options.insert("top_p".into(), json!(v));
                }
                if let Some(v) = req.params.max_tokens {
                    options.insert("num_predict".into(), json!(v));
                }
                if !req.params.stop.is_empty() {
                    options.insert("stop".into(), json!(req.params.stop));
                }
                for (k, v) in &req.params.extra {
                    options.insert(k.clone(), v.clone());
                }
                let body = json!({
                    "model": self.model,
                    "stream": false,
                    "options": options,
                    "messages": req.messages.iter().map(|m| json!({
                        "role": "user",
                        "content": m.parts.iter().map(|p| match p {
                            crate::core::llm::types::ContentPart::Text { text } => text.clone(),
                            _ => String::new(),
                        }).collect::<String>(),
                    })).collect::<Vec<_>>(),
                });
                let resp = self
                    .client
                    .post("http://127.0.0.1:11434/api/chat")
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| {
                        LlmError::new(crate::core::llm::error::ERR_LLM_NET, e.to_string())
                    })?;
                let raw = resp.text().await.unwrap_or_default();
                self.capture.record(redact(&body), &raw);
                let parsed: Value = serde_json::from_str(&raw)
                    .map_err(|e| LlmError::new(ERR_LLM_PARSE, e.to_string()))?;
                let text = parsed["message"]["content"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                Ok(stream::iter(vec![TurnEvent::TextDelta { text }]).boxed())
            }
            fn wire_capture(&self) -> Option<Arc<WireCapture>> {
                Some(self.capture.clone())
            }
        }

        let name = std::env::var("OMNIGET_PROBE_MODEL").unwrap_or_else(|_| "llama3.2:1b".into());
        let provider: Arc<dyn Provider> = Arc::new(Ollama {
            client: reqwest::Client::new(),
            capture: Arc::new(WireCapture::default()),
            model: name.clone(),
        });
        let m = ModelRef {
            provider: ProviderId::new("ollama"),
            model: name,
        };
        let out = run(provider, &m, &default_cases()).await;
        for r in &out {
            println!(
                "{:<18} {:<12} differs={} detail={}\n  A {}\n  B {}",
                r.param,
                r.verdict.as_str(),
                r.differs,
                r.detail,
                r.reply_a.replace('\n', " / "),
                r.reply_b.replace('\n', " / ")
            );
        }
        assert_eq!(out.len(), 4);
        assert!(
            out.iter().all(|r| r.verdict != Verdict::Unsupported),
            "Ollama should answer every case: {out:?}"
        );
    }
}
