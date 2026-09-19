//! OpenAI-compatible `/v1/chat/completions` + `/v1/models` on the local
//! bridge: with it on, the app is a provider of the machine and any client
//! that speaks OpenAI (the Jan model of the plan) can talk to an OmniGet agent
//! at `omniget/<agent_id>`. Owned by f2-llm-commands.
//!
//! Same bearer as the extension, and **off by default** (`llm_bridge_openai_enabled`):
//! with it on, whoever holds the token spends the user's tokens.
//!
//! The router carries its own state so it can be merged into the bridge with a
//! single line, and tested without a Tauri app.

use std::convert::Infallible;
use std::sync::Arc;

use axum::response::sse::{Event, Sse};
use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures::stream::{Stream, StreamExt};
use omniget_core::core::llm::types::{ContentPart, Message, Role, TurnEvent};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::llm_manager::LlmManager;

/// Model ids are namespaced so a client can tell them from a real provider's.
pub const MODEL_PREFIX: &str = "omniget/";

#[derive(Clone)]
pub struct LlmBridgeState {
    pub token: Arc<String>,
    pub manager: Arc<LlmManager>,
}

impl LlmBridgeState {
    pub fn new(token: Arc<String>, manager: Arc<LlmManager>) -> Self {
        Self { token, manager }
    }

    /// Built from the bridge's own state; used by the one merge line in
    /// `local_bridge.rs`.
    pub fn from_bridge(state: &crate::local_bridge::BridgeState) -> Self {
        use tauri::Manager;
        let app_state = state.app.state::<crate::AppState>();
        // The bridge can be the first caller, so it is also a place where the
        // tool executor gets its AppHandle and the bus forwarder starts.
        crate::commands::llm::ensure_wired(&state.app);
        Self {
            token: state.token.clone(),
            manager: app_state.llm.clone(),
        }
    }
}

/// The two OpenAI routes. Generic over the outer router's state because the
/// handlers close over `LlmBridgeState` instead of extracting it.
pub fn router<S>(state: LlmBridgeState) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let models_state = state.clone();
    Router::new()
        .route(
            "/v1/models",
            get(move |headers: HeaderMap| {
                let state = models_state.clone();
                async move { models(state, headers).await }
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |headers: HeaderMap, body: axum::body::Bytes| {
                let state = state.clone();
                async move { chat_completions(state, headers, body).await }
            }),
        )
}

// ── Wire types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    /// OpenAI allows a string or an array of parts; both land as text here.
    #[serde(default)]
    pub content: Value,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorDetail {
    message: String,
    code: String,
    #[serde(rename = "type")]
    kind: &'static str,
}

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(ErrorBody {
            error: ErrorDetail {
                message: message.into(),
                code: code.to_string(),
                kind: "invalid_request_error",
            },
        }),
    )
        .into_response()
}

/// Bearer check, constant time. A copy of the bridge's own (private) one so
/// this module stays independent of it.
pub fn check_bearer(headers: &HeaderMap, expected: &str) -> bool {
    let Some(raw) = headers.get("authorization").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(provided) = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
    else {
        return false;
    };
    constant_time_eq::constant_time_eq(provided.trim().as_bytes(), expected.as_bytes())
}

/// `omniget/<agent_id>` → `agent_id`. Pure.
pub fn agent_of_model(model: &str) -> Option<&str> {
    let id = model.strip_prefix(MODEL_PREFIX)?;
    (!id.is_empty()).then_some(id)
}

/// Flattens OpenAI's `content` (string, or array of `{type:"text",text}`).
pub fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Maps the OpenAI request into our own messages. The `system` role is kept:
/// the agent's own prompt is prepended by the manager.
pub fn to_messages(request: &ChatRequest) -> Vec<Message> {
    request
        .messages
        .iter()
        .map(|m| {
            let role = match m.role.as_str() {
                "system" | "developer" => Role::System,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => Role::User,
            };
            Message {
                role,
                parts: vec![ContentPart::Text {
                    text: content_text(&m.content),
                }],
            }
        })
        .collect()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn chunk_json(id: &str, model: &str, delta: Value, finish: Option<&str>) -> Value {
    json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": now_secs(),
        "model": model,
        "choices": [ { "index": 0, "delta": delta, "finish_reason": finish } ],
    })
}

// ── Handlers ──────────────────────────────────────────────────────────

async fn models(state: LlmBridgeState, headers: HeaderMap) -> Response {
    if !check_bearer(&headers, &state.token) {
        return error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "invalid bearer");
    }
    if !state.manager.bridge_openai_enabled() {
        return error(
            StatusCode::FORBIDDEN,
            "ERR_LLM_BRIDGE_OFF",
            "the OpenAI-compatible bridge is off (OmniGet → LLM → Local)",
        );
    }
    let created = now_secs();
    let data: Vec<Value> = state
        .manager
        .roster()
        .into_iter()
        .map(|a| {
            json!({
                "id": format!("{MODEL_PREFIX}{}", a.id),
                "object": "model",
                "created": created,
                "owned_by": "omniget",
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "object": "list", "data": data })),
    )
        .into_response()
}

async fn chat_completions(
    state: LlmBridgeState,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !check_bearer(&headers, &state.token) {
        return error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "invalid bearer");
    }
    if !state.manager.bridge_openai_enabled() {
        return error(
            StatusCode::FORBIDDEN,
            "ERR_LLM_BRIDGE_OFF",
            "the OpenAI-compatible bridge is off (OmniGet → LLM → Local)",
        );
    }
    let request: ChatRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error(StatusCode::BAD_REQUEST, "ERR_LLM_PARSE", e.to_string()),
    };
    let Some(agent_id) = agent_of_model(&request.model).map(str::to_string) else {
        return error(
            StatusCode::NOT_FOUND,
            "ERR_LLM_MODEL",
            format!(
                "model must be {MODEL_PREFIX}<agent_id>, got {}",
                request.model
            ),
        );
    };
    let messages = to_messages(&request);
    if messages.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "ERR_LLM_PARSE",
            "messages is empty",
        );
    }

    let turn = state.manager.turn_stateless(&agent_id, messages).await;
    let (request_id, conversation_id, _cancel, stream) = match turn {
        Ok(t) => t,
        Err(message) => {
            let status = if message.starts_with("ERR_LLM_NO_AGENT") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_GATEWAY
            };
            let code = message
                .split(':')
                .next()
                .unwrap_or("ERR_LLM_NET")
                .to_string();
            return error(status, &code, message);
        }
    };
    let completion_id = format!("chatcmpl-{request_id}");
    let model = request.model.clone();

    if request.stream {
        let sse = sse_stream(
            state.manager.clone(),
            agent_id,
            request_id,
            conversation_id,
            completion_id,
            model,
            stream,
        );
        return Sse::new(sse).into_response();
    }

    let manager = state.manager.clone();
    let mut stream = stream;
    let mut answer = String::new();
    let mut usage = json!({ "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0 });
    while let Some(event) = stream.next().await {
        manager.note_event(&agent_id, &event);
        match event {
            TurnEvent::TextDelta { text } => answer.push_str(&text),
            TurnEvent::Usage { usage: u } => {
                usage = json!({
                    "prompt_tokens": u.input_tokens,
                    "completion_tokens": u.output_tokens,
                    "total_tokens": u.input_tokens + u.output_tokens,
                });
            }
            TurnEvent::Error { error: e } => {
                manager.finish_turn(&request_id, &agent_id);
                manager.drop_conversation(&conversation_id);
                return error(StatusCode::BAD_GATEWAY, &e.code, e.message);
            }
            TurnEvent::Finished { .. } => break,
            _ => {}
        }
    }
    manager.finish_turn(&request_id, &agent_id);
    manager.drop_conversation(&conversation_id);

    (
        StatusCode::OK,
        Json(json!({
            "id": completion_id,
            "object": "chat.completion",
            "created": now_secs(),
            "model": model,
            "choices": [ {
                "index": 0,
                "message": { "role": "assistant", "content": answer },
                "finish_reason": "stop",
            } ],
            "usage": usage,
        })),
    )
        .into_response()
}

/// The unfold state: an explicit state machine instead of a macro crate, so
/// the chunk order is readable and no new dependency is pulled in.
struct SseState {
    manager: Arc<LlmManager>,
    agent_id: String,
    completion_id: String,
    model: String,
    stream: futures::stream::BoxStream<'static, TurnEvent>,
    request_id: String,
    conversation_id: String,
    sent_role: bool,
    done: bool,
}

impl SseState {
    /// End of turn: telemetry closed and the throwaway conversation removed.
    fn close(&mut self) {
        self.done = true;
        self.manager.finish_turn(&self.request_id, &self.agent_id);
        self.manager.drop_conversation(&self.conversation_id);
    }
}

/// Turn events → OpenAI chunks, ending with `data: [DONE]`.
#[allow(clippy::too_many_arguments)]
fn sse_stream(
    manager: Arc<LlmManager>,
    agent_id: String,
    request_id: String,
    conversation_id: String,
    completion_id: String,
    model: String,
    stream: futures::stream::BoxStream<'static, TurnEvent>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let state = SseState {
        manager,
        agent_id,
        request_id,
        conversation_id,
        completion_id,
        model,
        stream,
        sent_role: false,
        done: false,
    };
    futures::stream::unfold(state, |mut s| async move {
        if s.done {
            return None;
        }
        if !s.sent_role {
            s.sent_role = true;
            let chunk = chunk_json(
                &s.completion_id,
                &s.model,
                json!({ "role": "assistant", "content": "" }),
                None,
            );
            return Some((Ok(Event::default().data(chunk.to_string())), s));
        }
        while let Some(event) = s.stream.next().await {
            s.manager.note_event(&s.agent_id, &event);
            match event {
                TurnEvent::TextDelta { text } => {
                    let chunk =
                        chunk_json(&s.completion_id, &s.model, json!({ "content": text }), None);
                    return Some((Ok(Event::default().data(chunk.to_string())), s));
                }
                TurnEvent::Error { error } => {
                    let chunk =
                        json!({ "error": { "message": error.message, "code": error.code } });
                    s.close();
                    return Some((Ok(Event::default().data(chunk.to_string())), s));
                }
                TurnEvent::Finished { .. } => break,
                _ => continue,
            }
        }
        // Final chunk, then the sentinel every OpenAI client waits for.
        let stop = chunk_json(&s.completion_id, &s.model, json!({}), Some("stop"));
        s.close();
        Some((Ok(Event::default().data(stop.to_string())), s))
    })
    .chain(futures::stream::once(async {
        Ok(Event::default().data("[DONE]"))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manager of its own, with the single roster agent pinned to the fake
    /// provider so a turn runs without a key.
    fn state(enabled: bool) -> LlmBridgeState {
        state_with(enabled, crate::llm_manager::FAKE_PROVIDER, "fake")
    }

    /// A manager of its own, with the single roster agent pinned to one
    /// provider/model pair.
    fn state_with(enabled: bool, provider: &str, model: &str) -> LlmBridgeState {
        use omniget_core::core::llm::agent::ModelPolicy;
        use omniget_core::core::llm::roster_store;
        use omniget_core::core::llm::types::{ModelRef, ProviderId};

        let manager = LlmManager::new();
        manager.set_root(
            std::env::temp_dir().join(format!("omniget-bridge-{}", uuid::Uuid::new_v4())),
        );
        let mut agent = roster_store::default_roster().remove(0);
        agent.model = ModelPolicy::Fixed {
            model: ModelRef {
                provider: ProviderId::new(provider),
                model: model.to_string(),
            },
        };
        manager.roster_update(agent).expect("pin the provider");
        manager.set_bridge_openai(enabled);
        LlmBridgeState::new(Arc::new("test-token".to_string()), Arc::new(manager))
    }

    async fn serve(state: LlmBridgeState) -> String {
        let app: Router<()> = router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    /// Serves the router on a fixed port so a real `curl -N` can be run
    /// against it, which is what the phase gate asks for. Ignored by default;
    /// run with `OMNIGET_TEST_BRIDGE_PORT=47799 cargo test -p omniget
    /// local_bridge_llm::tests::serve_for_curl -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "serves a port for a manual curl; set OMNIGET_TEST_BRIDGE_PORT"]
    async fn serve_for_curl() {
        let port: u16 = std::env::var("OMNIGET_TEST_BRIDGE_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(47799);
        // `OMNIGET_TEST_BRIDGE_PROVIDER`/`_MODEL` pin a real provider (the
        // end-to-end run against Ollama uses ollama + qwen3:0.6b).
        let provider = std::env::var("OMNIGET_TEST_BRIDGE_PROVIDER")
            .unwrap_or_else(|_| crate::llm_manager::FAKE_PROVIDER.to_string());
        let model =
            std::env::var("OMNIGET_TEST_BRIDGE_MODEL").unwrap_or_else(|_| "fake".to_string());
        let app: Router<()> = router(state_with(true, &provider, &model));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("bind");
        println!(
            "bridge up on http://127.0.0.1:{port} with bearer test-token ({provider}/{model})"
        );
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let secs: u64 = std::env::var("OMNIGET_TEST_BRIDGE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(25);
        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        server.abort();
    }

    #[test]
    fn agent_of_model_only_accepts_the_prefix() {
        assert_eq!(agent_of_model("omniget/omni"), Some("omni"));
        assert_eq!(agent_of_model("gpt-4o-mini"), None);
        assert_eq!(agent_of_model("omniget/"), None);
    }

    #[test]
    fn content_text_handles_both_shapes() {
        assert_eq!(content_text(&json!("hi")), "hi");
        assert_eq!(
            content_text(
                &json!([{ "type": "text", "text": "a" }, { "type": "text", "text": "b" }])
            ),
            "ab"
        );
        assert_eq!(content_text(&json!(null)), "");
    }

    #[test]
    fn to_messages_maps_every_role() {
        let request: ChatRequest = serde_json::from_value(json!({
            "model": "omniget/omni",
            "messages": [
                { "role": "system", "content": "s" },
                { "role": "user", "content": "u" },
                { "role": "assistant", "content": "a" }
            ]
        }))
        .unwrap();
        let messages = to_messages(&request);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[1].role, Role::User);
        assert_eq!(messages[2].role, Role::Assistant);
    }

    #[tokio::test]
    async fn without_the_bearer_it_is_401() {
        let base = serve(state(true)).await;
        let r = reqwest::get(format!("{base}/v1/models")).await.unwrap();
        assert_eq!(r.status(), 401);
    }

    #[tokio::test]
    async fn with_the_bridge_off_it_is_403() {
        let base = serve(state(false)).await;
        let r = reqwest::Client::new()
            .get(format!("{base}/v1/models"))
            .bearer_auth("test-token")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403);
    }

    #[tokio::test]
    async fn models_lists_the_roster_namespaced() {
        let base = serve(state(true)).await;
        let body: Value = reqwest::Client::new()
            .get(format!("{base}/v1/models"))
            .bearer_auth("test-token")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let ids: Vec<&str> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"omniget/omni"), "{ids:?}");
    }

    #[tokio::test]
    async fn an_unknown_model_is_404() {
        let base = serve(state(true)).await;
        let r = reqwest::Client::new()
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth("test-token")
            .json(&json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "hi" }] }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 404);
    }

    #[tokio::test]
    async fn a_non_streaming_completion_answers_in_the_openai_shape() {
        let base = serve(state(true)).await;
        let body: Value = reqwest::Client::new()
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth("test-token")
            .json(&json!({
                "model": "omniget/omni",
                "messages": [{ "role": "user", "content": "hi" }]
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(body["object"], "chat.completion");
        assert_eq!(body["choices"][0]["message"]["role"], "assistant");
        assert!(!body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
            .is_empty());
        assert_eq!(body["choices"][0]["finish_reason"], "stop");
    }

    #[tokio::test]
    async fn a_streaming_completion_ends_with_done() {
        let base = serve(state(true)).await;
        let text = reqwest::Client::new()
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth("test-token")
            .json(&json!({
                "model": "omniget/omni",
                "stream": true,
                "messages": [{ "role": "user", "content": "hi" }]
            }))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(text.contains("chat.completion.chunk"), "{text}");
        assert!(text.contains("\"role\":\"assistant\""), "{text}");
        assert!(text.contains("\"finish_reason\":\"stop\""), "{text}");
        assert!(text.trim_end().ends_with("data: [DONE]"), "{text}");
        // Rebuilt answer from the deltas is not empty.
        let joined: String = text
            .lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .filter(|l| *l != "[DONE]")
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| {
                v["choices"][0]["delta"]["content"]
                    .as_str()
                    .map(String::from)
            })
            .collect();
        assert!(!joined.is_empty(), "no content in the stream: {text}");
    }
}
