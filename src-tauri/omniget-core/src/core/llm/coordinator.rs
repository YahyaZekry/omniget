//! Coordinator: owner of the turn. Streams the model through, resolves the
//! tool loop, cuts on budget, reroutes on rate limits without losing the turn,
//! persists the conversation and hands off between agents.
//! Owned by f2-llm-coordinator.
//!
//! The returned stream is the only thing the commands layer needs: every tool
//! call, reroute and retry already happened inside it. Everything the UI shows
//! beyond the text (tool asks, telemetry, the mascot) comes from the bus.
//!
//! Conversations are appended to `<app_data>/llm/conversations/<id>.jsonl`, one
//! JSON record per line (`OMNIGET_DATA_DIR` overrides the root).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use futures::channel::mpsc;
use futures::sink::SinkExt;
use futures::stream::{BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::agent::{AgentDef, ModelPolicy};
use super::broker::ToolBroker;
use super::budget::BudgetStore;
use super::error::LlmError;
use super::prune::ContextPruner;
use super::router::{self, AlwaysAvailable, Router};
use super::runtime::AgentRuntime;
use super::types::{
    ContentPart, FinishReason, GenParams, Message, ModelRef, Role, TurnEvent, TurnRequest, Usage,
};
use crate::core::omni::bus::{Bus, BusEvent};

/// The agent asked for more tool calls than its budget allows in one turn.
pub const ERR_LLM_TOOL_LIMIT: &str = "ERR_LLM_TOOL_LIMIT";
/// The conversation id is not a safe file name.
pub const ERR_LLM_CONVERSATION: &str = "ERR_LLM_CONVERSATION";

/// Used when `Budget::max_tool_calls_per_turn` is 0 (unset), so a fresh agent
/// can still use tools.
pub const DEFAULT_MAX_TOOL_CALLS: u8 = 48;
/// Buffer of the event channel handed to the caller.
const CHANNEL_CAP: usize = 64;

/// One line of `<id>.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationRecord {
    pub ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub message: Message,
}

/// `<app_data>/llm/conversations`.
pub fn default_conversations_dir() -> Option<PathBuf> {
    crate::core::paths::app_data_dir().map(|d| d.join("llm").join("conversations"))
}

/// Conversation ids become file names, so they are restricted to
/// `[A-Za-z0-9_-]` (uuids and slugs pass, `../` does not).
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn conversation_path(dir: &Path, id: &str) -> Result<PathBuf, LlmError> {
    if !is_safe_id(id) {
        return Err(LlmError::new(
            ERR_LLM_CONVERSATION,
            format!("unsafe conversation id `{id}`"),
        ));
    }
    Ok(dir.join(format!("{id}.jsonl")))
}

/// Cut a conversation back to its first `keep` messages and return what was
/// removed. Undo uses it: the tools of the removed turn are not run again.
pub fn truncate_conversation(dir: &Path, id: &str, keep: usize) -> Result<Vec<Message>, LlmError> {
    let path = conversation_path(dir, id)?;
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let mut kept = String::new();
    let mut removed = Vec::new();
    let mut seen = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(record) = serde_json::from_str::<ConversationRecord>(line) else {
            continue;
        };
        if seen < keep {
            kept.push_str(line);
            kept.push('\n');
        } else {
            removed.push(record.message);
        }
        seen += 1;
    }
    if !removed.is_empty() {
        std::fs::write(&path, kept)
            .map_err(|e| LlmError::new(ERR_LLM_CONVERSATION, e.to_string()))?;
    }
    Ok(removed)
}

/// Read a conversation back. Unparseable lines are skipped: a truncated file
/// must not take the chat down.
pub fn load_conversation(dir: &Path, id: &str) -> Result<Vec<Message>, LlmError> {
    let path = conversation_path(dir, id)?;
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    Ok(text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<ConversationRecord>(l).ok())
        .map(|r| r.message)
        .collect())
}

/// Append one message. Best effort: a read-only disk must not fail the turn.
pub fn append_message(dir: &Path, id: &str, agent: Option<&str>, message: &Message) {
    let Ok(path) = conversation_path(dir, id) else {
        return;
    };
    let record = ConversationRecord {
        ts: chrono::Utc::now().to_rfc3339(),
        agent: agent.map(|s| s.to_string()),
        message: message.clone(),
    };
    let Ok(mut line) = serde_json::to_string(&record) else {
        return;
    };
    line.push('\n');
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Rough token estimate for the budget gate: ~4 characters per token. Cheap on
/// purpose; the real number comes back in `Usage`.
/// The project brief and the KB index, folded into the first system message of
/// the request. Read fresh each time (an agent may have just written a note)
/// and never persisted, so the transcript does not grow with it.
fn with_project_kb(conversation_id: &str, history: &[Message]) -> Vec<Message> {
    let mut messages = history.to_vec();
    let Some(ws) = super::code_tools::workspace_of(conversation_id) else {
        return messages;
    };
    let index = super::kb::index_prompt(&ws);
    if index.trim().is_empty() {
        return messages;
    }
    match messages.iter_mut().find(|m| m.role == Role::System) {
        Some(system) => system.parts.push(ContentPart::Text {
            text: format!("\n\n{index}"),
        }),
        None => messages.insert(0, Message::text(Role::System, index)),
    }
    messages
}

/// Ceiling for one tool result of a CLI agent kept in the transcript.
const CLI_RESULT_MAX_CHARS: usize = 12_000;

fn clip_cli_result(content: &str) -> String {
    if content.len() <= CLI_RESULT_MAX_CHARS {
        return content.to_string();
    }
    let mut end = CLI_RESULT_MAX_CHARS;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[clipped: {} of {} chars kept]",
        &content[..end],
        end,
        content.len()
    )
}

pub fn estimate_tokens(messages: &[Message]) -> u32 {
    let chars: usize = messages
        .iter()
        .flat_map(|m| m.parts.iter())
        .map(|p| match p {
            ContentPart::Text { text } => text.len(),
            ContentPart::Image { data_b64, .. } => data_b64.len() / 3,
            ContentPart::ToolUse { input, .. } => input.to_string().len(),
            ContentPart::ToolResult { content, .. } => content.len(),
        })
        .sum();
    (chars / 4) as u32
}

/// A tool call being assembled from the stream deltas.
#[derive(Debug, Clone, Default)]
struct PendingCall {
    id: String,
    name: String,
    input_json: String,
}

impl PendingCall {
    fn input(&self) -> Value {
        serde_json::from_str(&self.input_json).unwrap_or(Value::Null)
    }
}

pub struct Coordinator {
    runtime: Arc<dyn AgentRuntime>,
    broker: Arc<ToolBroker>,
    budget: Arc<BudgetStore>,
    bus: Arc<Bus>,
    router: Arc<Router>,
    dir: Option<PathBuf>,
    params: GenParams,
    /// Context pruning. Disabled until someone calls `with_prune`, and even
    /// then it only ever runs between turns. Behind a lock so a settings
    /// change swaps it without rebuilding the coordinator; a turn in flight
    /// keeps the one it started with.
    pruner: std::sync::RwLock<Arc<ContextPruner>>,
}

impl std::fmt::Debug for Coordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Coordinator")
            .field("dir", &self.dir)
            .finish()
    }
}

impl Coordinator {
    pub fn new(
        runtime: Arc<dyn AgentRuntime>,
        broker: Arc<ToolBroker>,
        budget: Arc<BudgetStore>,
        bus: Arc<Bus>,
    ) -> Self {
        Self {
            runtime,
            broker,
            budget,
            bus,
            router: Arc::new(Router::new(Arc::new(AlwaysAvailable))),
            dir: default_conversations_dir(),
            params: GenParams::default(),
            pruner: std::sync::RwLock::new(Arc::new(ContextPruner::disabled())),
        }
    }

    pub fn with_router(mut self, router: Arc<Router>) -> Self {
        self.router = router;
        self
    }

    /// Installs context pruning. The pruner carries its own `PruneConfig`
    /// (default off) and its own judge, so nothing outside this call knows
    /// about pruning settings.
    pub fn with_prune(self, pruner: Arc<ContextPruner>) -> Self {
        self.set_prune(pruner);
        self
    }

    /// Same as [`with_prune`], on a coordinator that is already shared.
    pub fn set_prune(&self, pruner: Arc<ContextPruner>) {
        match self.pruner.write() {
            Ok(mut slot) => *slot = pruner,
            Err(poisoned) => *poisoned.into_inner() = pruner,
        }
    }

    pub fn pruner(&self) -> Arc<ContextPruner> {
        match self.pruner.read() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Where conversations are written; `None` keeps the turn in memory.
    pub fn with_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.dir = dir;
        self
    }

    pub fn with_params(mut self, params: GenParams) -> Self {
        self.params = params;
        self
    }

    pub fn bus(&self) -> &Arc<Bus> {
        &self.bus
    }

    pub fn broker(&self) -> &Arc<ToolBroker> {
        &self.broker
    }

    pub fn router(&self) -> &Arc<Router> {
        &self.router
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn history(&self, conversation_id: &str) -> Result<Vec<Message>, LlmError> {
        match &self.dir {
            Some(dir) => load_conversation(dir, conversation_id),
            None => Ok(Vec::new()),
        }
    }

    fn persist(&self, conversation_id: &str, agent: &str, message: &Message) {
        if let Some(dir) = &self.dir {
            append_message(dir, conversation_id, Some(agent), message);
        }
    }

    /// Which model serves the next attempt, plus its routing key. `tried` holds
    /// the keys already burned in this turn.
    fn resolve(&self, agent: &AgentDef, tried: &[String]) -> Result<(ModelRef, String), LlmError> {
        match &agent.model {
            ModelPolicy::Fixed { model } => Ok((
                model.clone(),
                format!("native:{}:{}", model.provider.as_str(), model.model),
            )),
            ModelPolicy::Route { chain } => {
                let route =
                    self.router
                        .pick_for(chain, tried, router::TaskKind::of_agent(agent))?;
                Ok((route.model, route.key))
            }
        }
    }

    /// Hand the conversation from one agent to another with a written summary.
    /// The summary lands in the transcript as a system message, so the next
    /// agent's first turn already carries it.
    pub fn handoff(
        &self,
        conversation_id: &str,
        from: &AgentDef,
        to: &AgentDef,
        summary: &str,
    ) -> Result<Message, LlmError> {
        if let Some(dir) = &self.dir {
            conversation_path(dir, conversation_id)?;
        }
        let message = Message::text(
            Role::System,
            format!(
                "Handoff from {} ({}) to {} ({}). Summary of the work so far:\n{}",
                from.name, from.id, to.name, to.id, summary
            ),
        );
        self.persist(conversation_id, &to.id, &message);
        Ok(message)
    }

    /// Run one user turn. The returned stream already has the tool loop, the
    /// reroutes and the cancellation resolved.
    pub fn run_turn(
        self: &Arc<Self>,
        conversation_id: &str,
        agent: &AgentDef,
        user_input: &str,
        cancel: CancellationToken,
    ) -> BoxStream<'static, TurnEvent> {
        self.run_turn_with_id(
            uuid::Uuid::new_v4().to_string(),
            conversation_id,
            agent,
            user_input,
            cancel,
        )
    }

    /// Same turn, with the caller's own id. The commands layer mints the id it
    /// already handed to the front end and passes it here, so
    /// `TurnEvent::Started { request_id }` and `BusEvent::ToolAsk.request_id`
    /// carry that id and the UI can tie an ask back to its turn.
    pub fn run_turn_with_id(
        self: &Arc<Self>,
        request_id: String,
        conversation_id: &str,
        agent: &AgentDef,
        user_input: &str,
        cancel: CancellationToken,
    ) -> BoxStream<'static, TurnEvent> {
        let (tx, rx) = mpsc::channel::<TurnEvent>(CHANNEL_CAP);
        let this = self.clone();
        let conversation_id = conversation_id.to_string();
        let agent = agent.clone();
        let user_input = user_input.to_string();
        tokio::spawn(async move {
            this.drive(request_id, conversation_id, agent, user_input, cancel, tx)
                .await;
        });
        rx.boxed()
    }

    async fn drive(
        self: Arc<Self>,
        request_id: String,
        conversation_id: String,
        agent: AgentDef,
        user_input: String,
        cancel: CancellationToken,
        mut tx: mpsc::Sender<TurnEvent>,
    ) {
        let started = Instant::now();
        let mut history = self.history(&conversation_id).unwrap_or_default();
        super::snapshot::mark_turn(&request_id, history.len());
        // Replay the decisions a previous turn already froze. This is a map
        // lookup and a string swap, never a judge call: if the judgement of
        // the last turn is still running, this turn simply uses the history as
        // it stands and picks the omissions up next time.
        //
        // `history` itself stays whole: the omissions are applied to the copy
        // each request is built from (see the loop below), so the judge always
        // sees the real outputs and a verdict that lands in the middle of a
        // long turn takes effect on the very next request of that turn.
        if history.is_empty() && !agent.system_prompt.is_empty() {
            // Persisted too, so a resumed conversation replays identically.
            let system = Message::text(Role::System, agent.system_prompt.clone());
            self.persist(&conversation_id, &agent.id, &system);
            history.push(system);
        }
        let user_msg = Message::text(Role::User, user_input);
        self.persist(&conversation_id, &agent.id, &user_msg);
        history.push(user_msg);

        // Gate before anything reaches the network.
        let estimate = estimate_tokens(&history);
        if let Err(err) = self.budget.check(&agent.id, &agent.budget, estimate) {
            self.bus.emit(BusEvent::BudgetHit {
                agent: agent.id.clone(),
            });
            let _ = tx.send(TurnEvent::Error { error: err }).await;
            let _ = tx
                .send(TurnEvent::Finished {
                    reason: FinishReason::Other,
                })
                .await;
            return;
        }

        self.bus.emit(BusEvent::TurnStarted {
            agent: agent.id.clone(),
            conversation: conversation_id.clone(),
        });
        if tx
            .send(TurnEvent::Started {
                request_id: request_id.clone(),
            })
            .await
            .is_err()
        {
            return;
        }

        let specs = self.broker.specs_for(&agent.tools);
        let max_tool_calls = match agent.budget.max_tool_calls_per_turn {
            0 => DEFAULT_MAX_TOOL_CALLS,
            n => n,
        };
        let max_reroutes = match &agent.model {
            ModelPolicy::Route { chain } => chain.len(),
            ModelPolicy::Fixed { .. } => 0,
        };

        let mut tried: Vec<String> = Vec::new();
        let mut reroutes = 0usize;
        let mut tool_calls_used = 0u8;
        let mut total = Usage::default();
        let mut requests = 0u32;
        let finish: FinishReason = 'turn: loop {
            if cancel.is_cancelled() {
                break 'turn FinishReason::Cancelled;
            }
            let (model, key) = match self.resolve(&agent, &tried) {
                Ok(v) => v,
                Err(err) => {
                    let _ = tx.send(TurnEvent::Error { error: err }).await;
                    break 'turn FinishReason::Other;
                }
            };
            let pruner = self.pruner();
            let (sent, pruned) = pruner.apply_sticky(&conversation_id, &history);
            let receipt = pruner.config().enabled.then(|| {
                (
                    pruned.omitted as u32,
                    estimate_tokens(&history),
                    estimate_tokens(&sent),
                )
            });
            requests += 1;
            let req = TurnRequest {
                model,
                messages: with_project_kb(&conversation_id, &sent),
                tools: specs.clone(),
                params: self.params.clone(),
                cancel: cancel.clone(),
                agent_id: Some(agent.id.clone()),
            };
            // A CLI or ACP agent edits files with its own tools, which never
            // pass through the broker: the undo snapshot is taken up front.
            if !matches!(agent.runtime, super::agent::RuntimeKind::Native) {
                if let Some(ws) = super::code_tools::workspace_of(&conversation_id) {
                    if let Err(e) =
                        super::snapshot::before_write(&ws, &conversation_id, &request_id).await
                    {
                        tracing::warn!("[snapshot] {e}");
                    }
                }
            }
            let stream = match super::code_tools::scope(
                &conversation_id,
                &agent.id,
                &request_id,
                self.runtime.turn(&agent, req),
            )
            .await
            {
                Ok(s) => Some(s),
                Err(err) => {
                    match self.maybe_reroute(
                        &agent,
                        &err,
                        &key,
                        &mut tried,
                        &mut reroutes,
                        max_reroutes,
                    ) {
                        true => continue 'turn,
                        false => {
                            let _ = tx.send(TurnEvent::Error { error: err }).await;
                            break 'turn FinishReason::Other;
                        }
                    }
                }
            };
            let mut stream = stream.expect("stream or an early continue");

            let mut text = String::new();
            let mut calls: Vec<PendingCall> = Vec::new();
            let mut failed: Option<LlmError> = None;
            let mut reason: Option<FinishReason> = None;
            let mut cancelled = false;
            let mut request_input: Option<u32> = None;
            let mut cli_results: Vec<(String, String, bool)> = Vec::new();

            loop {
                let next = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        cancelled = true;
                        None
                    }
                    ev = stream.next() => ev,
                };
                let Some(event) = next else { break };
                match event {
                    TurnEvent::Started { .. } => {}
                    TurnEvent::TextDelta { text: delta } => {
                        self.bus.emit(BusEvent::TokenDelta {
                            agent: agent.id.clone(),
                            chars: delta.chars().count() as u32,
                        });
                        text.push_str(&delta);
                        if tx.send(TurnEvent::TextDelta { text: delta }).await.is_err() {
                            cancelled = true;
                            break;
                        }
                    }
                    TurnEvent::ThinkingDelta { text: delta } => {
                        let _ = tx.send(TurnEvent::ThinkingDelta { text: delta }).await;
                    }
                    TurnEvent::ToolCallStart { id, name } => {
                        calls.push(PendingCall {
                            id: id.clone(),
                            name: name.clone(),
                            input_json: String::new(),
                        });
                        let _ = tx.send(TurnEvent::ToolCallStart { id, name }).await;
                    }
                    TurnEvent::ToolCallDelta {
                        id,
                        input_json_delta,
                    } => {
                        if let Some(c) = calls.iter_mut().find(|c| c.id == id) {
                            c.input_json.push_str(&input_json_delta);
                        }
                        let _ = tx
                            .send(TurnEvent::ToolCallDelta {
                                id,
                                input_json_delta,
                            })
                            .await;
                    }
                    TurnEvent::ToolCallEnd { id } => {
                        let _ = tx.send(TurnEvent::ToolCallEnd { id }).await;
                    }
                    TurnEvent::ToolResult {
                        id,
                        content,
                        is_error,
                    } => {
                        cli_results.push((id, content, is_error));
                    }
                    // Only the Coordinator writes receipts.
                    TurnEvent::PruneReceipt { .. } => {}
                    TurnEvent::Usage { usage } => {
                        request_input = Some(
                            request_input.unwrap_or(0)
                                + usage.input_tokens
                                + usage.cache_read_tokens
                                + usage.cache_write_tokens,
                        );
                        total.input_tokens += usage.input_tokens;
                        total.output_tokens += usage.output_tokens;
                        total.cache_read_tokens += usage.cache_read_tokens;
                        total.cache_write_tokens += usage.cache_write_tokens;
                        if total.first_token_ms.is_none() {
                            total.first_token_ms = usage.first_token_ms;
                        }
                        if let Some(c) = usage.cost_usd {
                            total.cost_usd = Some(total.cost_usd.unwrap_or(0.0) + c);
                        }
                    }
                    TurnEvent::Finished { reason: r } => {
                        reason = Some(r);
                        break;
                    }
                    TurnEvent::Error { error } => {
                        failed = Some(error);
                        break;
                    }
                }
            }

            if let Some((omitted, before, after)) = receipt {
                let _ = tx
                    .send(TurnEvent::PruneReceipt {
                        request: requests,
                        omitted,
                        est_tokens_before: before,
                        est_tokens_after: after,
                        input_tokens: request_input,
                    })
                    .await;
            }

            if cancelled || cancel.is_cancelled() {
                break 'turn FinishReason::Cancelled;
            }

            if let Some(err) = failed {
                if self.maybe_reroute(&agent, &err, &key, &mut tried, &mut reroutes, max_reroutes) {
                    continue 'turn;
                }
                let _ = tx.send(TurnEvent::Error { error: err }).await;
                break 'turn FinishReason::Other;
            }

            // A CLI agent (Claude Code, Codex, ACP) runs its own tools inside
            // its own process: what it streamed is a record of work already
            // done, not a request for the broker. Re-running it would call
            // tools we do not have and start the CLI all over again.
            if !matches!(agent.runtime, super::agent::RuntimeKind::Native) {
                // What it did is still worth remembering: the next turn replays
                // the conversation, and without these pairs the CLI reads the
                // same files again. Only complete pairs are kept (a call with
                // no result would be an invalid request for a native model),
                // each result clipped, and from there on they are ordinary
                // candidates for the pruner.
                let pairs: Vec<(&PendingCall, &(String, String, bool))> = calls
                    .iter()
                    .filter_map(|c| cli_results.iter().find(|r| r.0 == c.id).map(|r| (c, r)))
                    .collect();
                if !pairs.is_empty() {
                    let asked = Message {
                        role: Role::Assistant,
                        parts: pairs
                            .iter()
                            .map(|(c, _)| ContentPart::ToolUse {
                                id: c.id.clone(),
                                name: c.name.clone(),
                                input: c.input(),
                            })
                            .collect(),
                    };
                    let answered = Message {
                        role: Role::Tool,
                        parts: pairs
                            .iter()
                            .map(|(_, r)| ContentPart::ToolResult {
                                tool_use_id: r.0.clone(),
                                content: clip_cli_result(&r.1),
                                is_error: r.2,
                            })
                            .collect(),
                    };
                    for msg in [asked, answered] {
                        self.persist(&conversation_id, &agent.id, &msg);
                        history.push(msg);
                    }
                }
                calls.clear();
            }

            if calls.is_empty() {
                if !text.is_empty() {
                    let msg = Message::text(Role::Assistant, text);
                    self.persist(&conversation_id, &agent.id, &msg);
                    history.push(msg);
                }
                break 'turn reason.unwrap_or(FinishReason::Stop);
            }

            if tool_calls_used as usize + calls.len() > max_tool_calls as usize {
                let _ = tx
                    .send(TurnEvent::Error {
                        error: LlmError::new(
                            ERR_LLM_TOOL_LIMIT,
                            format!(
                                "agent `{}` hit {max_tool_calls} tool calls in one turn",
                                agent.id
                            ),
                        ),
                    })
                    .await;
                break 'turn FinishReason::Other;
            }

            // The assistant message that asked for the tools, then one tool
            // message per result; both go to disk so a resumed conversation
            // replays the same loop.
            let mut parts: Vec<ContentPart> = Vec::new();
            if !text.is_empty() {
                parts.push(ContentPart::Text { text: text.clone() });
            }
            for c in &calls {
                parts.push(ContentPart::ToolUse {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    input: c.input(),
                });
            }
            let assistant = Message {
                role: Role::Assistant,
                parts,
            };
            self.persist(&conversation_id, &agent.id, &assistant);
            history.push(assistant);

            for c in &calls {
                if cancel.is_cancelled() {
                    break 'turn FinishReason::Cancelled;
                }
                let outcome = super::code_tools::scope(
                    &conversation_id,
                    &agent.id,
                    &request_id,
                    self.broker.call(
                        &agent.id,
                        &agent.tools,
                        &request_id,
                        &c.id,
                        &c.name,
                        c.input(),
                    ),
                )
                .await;
                let (content, is_error) = match outcome {
                    Ok(o) => (super::compress::tool_output(&c.name, &o.content), false),
                    Err(e) => (format!("{}: {}", e.code, e.message), true),
                };
                let msg = Message {
                    role: Role::Tool,
                    parts: vec![ContentPart::ToolResult {
                        tool_use_id: c.id.clone(),
                        content,
                        is_error,
                    }],
                };
                self.persist(&conversation_id, &agent.id, &msg);
                history.push(msg);
                tool_calls_used += 1;
            }

            // A long turn is many requests. The judge looks at what is already
            // old inside this same turn, in its own task; whatever it freezes
            // is applied to the next request above. Never awaited here.
            if pruner.is_active() {
                let (pruner, id, snapshot) =
                    (pruner.clone(), conversation_id.clone(), history.clone());
                tokio::spawn(async move {
                    pruner.judge_turn(&id, &snapshot).await;
                });
            }
        };

        total.total_ms = started.elapsed().as_millis() as u32;
        if !matches!(finish, FinishReason::Cancelled) {
            let _ = tx
                .send(TurnEvent::Usage {
                    usage: total.clone(),
                })
                .await;
        }
        let _ = tx.send(TurnEvent::Finished { reason: finish }).await;
        self.budget.record(
            &agent.id,
            total.cost_usd,
            total.input_tokens,
            total.output_tokens,
        );
        self.bus.emit(BusEvent::TurnEnded {
            agent: agent.id.clone(),
            usage: total,
        });

        // Context pruning runs here and only here: the turn is over, the
        // stream is closed, and the judge — a model or a network call — gets
        // its own task so nothing in the SSE path ever awaits it. The verdicts
        // land in the sticky store and take effect on the next turn.
        let pruner = self.pruner();
        if pruner.is_active() {
            let id = conversation_id;
            tokio::spawn(async move {
                pruner.judge_turn(&id, &history).await;
            });
        }
    }

    /// Rate limited? Burn this candidate, put it on cooldown and tell the bus
    /// where the turn moved. The conversation is resent whole by the caller.
    fn maybe_reroute(
        &self,
        agent: &AgentDef,
        err: &LlmError,
        key: &str,
        tried: &mut Vec<String>,
        reroutes: &mut usize,
        max_reroutes: usize,
    ) -> bool {
        if !router::is_reroutable(&err.code) || *reroutes >= max_reroutes {
            return false;
        }
        let ModelPolicy::Route { chain } = &agent.model else {
            return false;
        };
        tried.push(key.to_string());
        self.router.rotate_away(key);
        let Ok(next) = self
            .router
            .pick_for(chain, tried, router::TaskKind::of_agent(agent))
        else {
            return false;
        };
        *reroutes += 1;
        self.bus.emit(BusEvent::Rerouted {
            agent: agent.id.clone(),
            from: key.to_string(),
            to: next.key,
            why: err.code.to_string(),
        });
        true
    }
}

#[cfg(test)]
#[path = "tests/coordinator_fake.rs"]
mod coordinator_fake;

/// The pruning hook only: what the model is actually sent, and that the turn
/// never waits for a judge. Kept apart from `coordinator_fake` because that
/// file belongs to another owner.
#[cfg(test)]
mod prune_hook_tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::core::llm::agent::{AgentRole, Budget, RuntimeKind};
    use crate::core::llm::broker::ToolExecutor;
    use crate::core::llm::providers::fake::FakeProvider;
    use crate::core::llm::providers::Provider;
    use crate::core::llm::prune::judge::fake::FakeJudge;
    use crate::core::llm::prune::{ContextPruner, KeepVerdict, PruneConfig};
    use crate::core::llm::types::{ProviderId, ToolSpec};
    use crate::core::omni::bus::Bus;

    /// Records the history every attempt was given, then answers with one
    /// short text. `delay` makes the turn outlast a slow judge.
    struct CapturingRuntime {
        sent: Mutex<Vec<Vec<Message>>>,
    }

    #[async_trait]
    impl AgentRuntime for CapturingRuntime {
        async fn turn(
            &self,
            _agent: &AgentDef,
            req: TurnRequest,
        ) -> Result<BoxStream<'static, TurnEvent>, LlmError> {
            self.sent.lock().unwrap().push(req.messages.clone());
            FakeProvider::new(vec![
                TurnEvent::TextDelta { text: "ok".into() },
                TurnEvent::Finished {
                    reason: FinishReason::Stop,
                },
            ])
            .turn(req)
            .await
        }
    }

    struct EchoExecutor;

    #[async_trait]
    impl ToolExecutor for EchoExecutor {
        async fn execute(&self, name: &str, input: Value) -> Result<String, LlmError> {
            Ok(format!("{name} ran with {input}"))
        }
    }

    fn agent() -> AgentDef {
        AgentDef {
            id: "worker".into(),
            name: "Worker".into(),
            role: AgentRole::Worker,
            system_prompt: "you are a test".into(),
            model: ModelPolicy::Fixed {
                model: ModelRef {
                    provider: ProviderId::new("fake"),
                    model: "m".into(),
                },
            },
            tools: vec![],
            skills: vec![],
            budget: Budget::default(),
            runtime: RuntimeKind::Native,
            skin: None,
        }
    }

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("omniget-prune-hook-{}", uuid::Uuid::new_v4()))
    }

    /// A conversation on disk whose middle holds one fat, complete tool pair.
    fn seed(dir: &Path, id: &str) {
        append_message(
            dir,
            id,
            Some("worker"),
            &Message::text(Role::System, "you are a test"),
        );
        append_message(
            dir,
            id,
            Some("worker"),
            &Message::text(Role::User, "consertar o parser"),
        );
        append_message(
            dir,
            id,
            Some("worker"),
            &Message {
                role: Role::Assistant,
                parts: vec![ContentPart::ToolUse {
                    id: "t1".into(),
                    name: "Read".into(),
                    input: json!({ "path": "/tmp/t1.txt" }),
                }],
            },
        );
        append_message(
            dir,
            id,
            Some("worker"),
            &Message {
                role: Role::Tool,
                parts: vec![ContentPart::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "SEGREDO".repeat(600),
                    is_error: false,
                }],
            },
        );
        for i in 0..8 {
            append_message(
                dir,
                id,
                Some("worker"),
                &Message::text(Role::Assistant, format!("passo {i}")),
            );
        }
    }

    fn coordinator(
        dir: &Path,
        pruner: Arc<ContextPruner>,
    ) -> (Arc<Coordinator>, Arc<CapturingRuntime>) {
        let bus = Arc::new(Bus::new());
        let runtime = Arc::new(CapturingRuntime {
            sent: Mutex::new(Vec::new()),
        });
        let broker = Arc::new(ToolBroker::new(
            Vec::<ToolSpec>::new(),
            Arc::new(EchoExecutor),
            bus.clone(),
        ));
        let coordinator = Coordinator::new(
            runtime.clone() as Arc<dyn AgentRuntime>,
            broker,
            Arc::new(BudgetStore::memory()),
            bus,
        )
        .with_dir(Some(dir.to_path_buf()))
        .with_prune(pruner);
        (Arc::new(coordinator), runtime)
    }

    fn enabled_pruner(dir: &Path, verdict: KeepVerdict) -> Arc<ContextPruner> {
        Arc::new(
            ContextPruner::new(
                PruneConfig {
                    enabled: true,
                    min_tokens: 0,
                    ..PruneConfig::default()
                },
                Some(Arc::new(FakeJudge::new(vec![("t1", verdict)]))),
            )
            .with_dir(Some(dir.to_path_buf())),
        )
    }

    #[tokio::test]
    async fn sem_prune_o_historico_chega_inteiro_ao_modelo() {
        let dir = temp_dir();
        seed(&dir, "conv");
        let (coordinator, runtime) = coordinator(&dir, Arc::new(ContextPruner::disabled()));
        let events: Vec<TurnEvent> = coordinator
            .run_turn("conv", &agent(), "e agora?", CancellationToken::new())
            .collect()
            .await;
        assert!(matches!(
            events.last(),
            Some(TurnEvent::Finished {
                reason: FinishReason::Stop
            })
        ));
        let sent = serde_json::to_string(&runtime.sent.lock().unwrap()[0]).unwrap();
        assert!(sent.contains("SEGREDOSEGREDO"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn desfazer_corta_a_conversa_no_inicio_do_turno_e_devolve_o_que_saiu() {
        let dir = temp_dir();
        seed(&dir, "conv");
        let before = load_conversation(&dir, "conv").unwrap().len();
        let removed = truncate_conversation(&dir, "conv", 2).unwrap();
        assert_eq!(removed.len(), before - 2);
        let kept = load_conversation(&dir, "conv").unwrap();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[1].role, Role::User);
        // Nothing past the end: a no-op, the file is left alone.
        assert!(truncate_conversation(&dir, "conv", 99).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cada_request_com_poda_ligada_sai_com_um_recibo_medido() {
        let dir = temp_dir();
        seed(&dir, "conv");
        let pruner = enabled_pruner(&dir, KeepVerdict::DROP_RESULT);
        let history = load_conversation(&dir, "conv").unwrap();
        pruner.judge_turn("conv", &history).await;
        let (coordinator, _runtime) = coordinator(&dir, pruner);
        let events: Vec<TurnEvent> = coordinator
            .run_turn("conv", &agent(), "e agora?", CancellationToken::new())
            .collect()
            .await;
        let receipt = events
            .iter()
            .find_map(|e| match e {
                TurnEvent::PruneReceipt {
                    request,
                    omitted,
                    est_tokens_before,
                    est_tokens_after,
                    ..
                } => Some((*request, *omitted, *est_tokens_before, *est_tokens_after)),
                _ => None,
            })
            .expect("a receipt per request");
        assert_eq!((receipt.0, receipt.1), (1, 1));
        assert!(receipt.3 < receipt.2, "{receipt:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn o_turno_seguinte_ve_o_marcador_e_o_par_continua_valido() {
        let dir = temp_dir();
        seed(&dir, "conv");
        let pruner = enabled_pruner(&dir, KeepVerdict::DROP_RESULT);
        // The judging pass a finished turn would have spawned.
        let history = load_conversation(&dir, "conv").unwrap();
        pruner.judge_turn("conv", &history).await;

        let (coordinator, runtime) = coordinator(&dir, pruner);
        let _: Vec<TurnEvent> = coordinator
            .run_turn("conv", &agent(), "e agora?", CancellationToken::new())
            .collect()
            .await;
        let sent = &runtime.sent.lock().unwrap()[0];
        let text = serde_json::to_string(sent).unwrap();
        assert!(!text.contains("SEGREDOSEGREDO"), "the payload must be gone");
        assert!(text.contains("[omitted by context pruning: tool_result t1"));
        // The pair is intact: one tool_use with its id, name and path, and one
        // tool_result answering it.
        assert!(text.contains("\"id\":\"t1\""));
        assert!(text.contains("/tmp/t1.txt"));
        assert!(text.contains("\"tool_use_id\":\"t1\""));
        // The transcript on disk still holds the original bytes.
        let disk = serde_json::to_string(&load_conversation(&dir, "conv").unwrap()).unwrap();
        assert!(disk.contains("SEGREDOSEGREDO"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The streaming loop must finish without waiting for the judge. The judge
    /// here sleeps far longer than the turn; the turn still ends immediately.
    #[tokio::test]
    async fn o_stream_nao_espera_o_juiz() {
        struct SlowJudge;

        #[async_trait]
        impl crate::core::llm::prune::ContextJudge for SlowJudge {
            fn name(&self) -> &'static str {
                "slow"
            }
            async fn judge(
                &self,
                _goal: &crate::core::llm::prune::GoalContext,
                _candidates: &[crate::core::llm::prune::Candidate],
            ) -> crate::core::llm::prune::Verdicts {
                tokio::time::sleep(Duration::from_secs(30)).await;
                crate::core::llm::prune::Verdicts::new()
            }
        }

        let dir = temp_dir();
        seed(&dir, "conv");
        let pruner = Arc::new(
            ContextPruner::new(
                PruneConfig {
                    enabled: true,
                    min_tokens: 0,
                    ..PruneConfig::default()
                },
                Some(Arc::new(SlowJudge)),
            )
            .with_dir(Some(dir.clone())),
        );
        let (coordinator, _runtime) = coordinator(&dir, pruner);
        let turn = coordinator.run_turn("conv", &agent(), "e agora?", CancellationToken::new());
        let events = tokio::time::timeout(Duration::from_secs(5), turn.collect::<Vec<TurnEvent>>())
            .await
            .expect("the turn must not wait for the judge");
        assert!(matches!(
            events.last(),
            Some(TurnEvent::Finished {
                reason: FinishReason::Stop
            })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
