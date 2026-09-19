//! Coordinator against `FakeProvider`: the whole turn loop without a network.
//! Included from `coordinator.rs`, so the path is `llm::coordinator::…`.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use super::*;
use crate::core::llm::agent::{
    AgentRole, Budget, Candidate, CandidateRuntime, GrantMode, ToolGrant, ToolSource,
};
use crate::core::llm::broker::ToolExecutor;
use crate::core::llm::error::{ERR_LLM_BUDGET, ERR_LLM_RATE};
use crate::core::llm::providers::fake::FakeProvider;
use crate::core::llm::providers::Provider;
use crate::core::llm::router::{
    Capacity, CapacityError, RouterConfig, StaticCapacity, ERR_CLI_RATE,
};
use crate::core::llm::types::{ProviderId, ToolSpec};

/// Replays one script per attempt, recording which model each attempt used.
struct ScriptedRuntime {
    scripts: Mutex<VecDeque<Vec<TurnEvent>>>,
    seen: Mutex<Vec<String>>,
    delay: Duration,
}

impl ScriptedRuntime {
    fn new(scripts: Vec<Vec<TurnEvent>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            seen: Mutex::new(Vec::new()),
            delay: Duration::ZERO,
        }
    }

    fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    fn attempts(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[async_trait]
impl AgentRuntime for ScriptedRuntime {
    async fn turn(
        &self,
        _agent: &AgentDef,
        req: TurnRequest,
    ) -> Result<BoxStream<'static, TurnEvent>, LlmError> {
        self.seen.lock().unwrap().push(format!(
            "{}:{}",
            req.model.provider.as_str(),
            req.model.model
        ));
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_else(|| {
            vec![TurnEvent::Finished {
                reason: FinishReason::Stop,
            }]
        });
        FakeProvider::new(script)
            .with_delay(self.delay)
            .turn(req)
            .await
    }
}

struct EchoExecutor;

#[async_trait]
impl ToolExecutor for EchoExecutor {
    async fn execute(&self, name: &str, input: serde_json::Value) -> Result<String, LlmError> {
        Ok(format!("{name} ran with {input}"))
    }
}

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: name.into(),
        input_schema: json!({ "type": "object" }),
    }
}

fn text_script(answer: &str) -> Vec<TurnEvent> {
    vec![
        TurnEvent::Started {
            request_id: "r".into(),
        },
        TurnEvent::TextDelta {
            text: answer.into(),
        },
        TurnEvent::Usage {
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cost_usd: Some(0.01),
                ..Usage::default()
            },
        },
        TurnEvent::Finished {
            reason: FinishReason::Stop,
        },
    ]
}

fn tool_script(id: &str, name: &str, input: &str) -> Vec<TurnEvent> {
    vec![
        TurnEvent::Started {
            request_id: "r".into(),
        },
        TurnEvent::ToolCallStart {
            id: id.into(),
            name: name.into(),
        },
        TurnEvent::ToolCallDelta {
            id: id.into(),
            input_json_delta: input.into(),
        },
        TurnEvent::ToolCallEnd { id: id.into() },
        TurnEvent::Finished {
            reason: FinishReason::ToolUse,
        },
    ]
}

fn rate_limited() -> Vec<TurnEvent> {
    vec![
        TurnEvent::Started {
            request_id: "r".into(),
        },
        TurnEvent::Error {
            error: LlmError::new(ERR_LLM_RATE, "slow down"),
        },
    ]
}

fn native(provider: &str, model: &str) -> Candidate {
    Candidate {
        runtime: CandidateRuntime::Native {
            provider: ProviderId::new(provider),
        },
        model: model.into(),
        max_cost_per_1k: None,
        min_context: 0,
    }
}

fn agent_with(policy: ModelPolicy, tools: Vec<ToolGrant>, budget: Budget) -> AgentDef {
    AgentDef {
        id: "worker".into(),
        name: "Worker".into(),
        role: AgentRole::Worker,
        system_prompt: "you are a test".into(),
        model: policy,
        tools,
        skills: vec![],
        budget,
        runtime: crate::core::llm::agent::RuntimeKind::Native,
        skin: None,
    }
}

fn fixed_agent() -> AgentDef {
    agent_with(
        ModelPolicy::Fixed {
            model: ModelRef {
                provider: ProviderId::new("fake"),
                model: "m".into(),
            },
        },
        vec![],
        Budget::default(),
    )
}

fn auto_grant(name: &str) -> ToolGrant {
    ToolGrant {
        source: ToolSource::Internal { name: name.into() },
        mode: GrantMode::Auto,
    }
}

struct Harness {
    coordinator: Arc<Coordinator>,
    bus: Arc<Bus>,
    runtime: Arc<ScriptedRuntime>,
    budget: Arc<BudgetStore>,
    dir: Option<PathBuf>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn harness(runtime: Arc<ScriptedRuntime>, persist: bool) -> Harness {
    harness_with(runtime, persist, vec![spec("dl_add")], None)
}

fn harness_with(
    runtime: Arc<ScriptedRuntime>,
    persist: bool,
    specs: Vec<ToolSpec>,
    router: Option<Arc<Router>>,
) -> Harness {
    let bus = Arc::new(Bus::new());
    let broker = Arc::new(ToolBroker::new(specs, Arc::new(EchoExecutor), bus.clone()));
    let budget = Arc::new(BudgetStore::memory());
    let dir = persist
        .then(|| std::env::temp_dir().join(format!("omniget-conv-{}", uuid::Uuid::new_v4())));
    let mut coordinator = Coordinator::new(
        runtime.clone() as Arc<dyn AgentRuntime>,
        broker,
        budget.clone(),
        bus.clone(),
    )
    .with_dir(dir.clone());
    if let Some(r) = router {
        coordinator = coordinator.with_router(r);
    }
    Harness {
        coordinator: Arc::new(coordinator),
        bus,
        runtime,
        budget,
        dir,
    }
}

async fn collect(stream: BoxStream<'static, TurnEvent>) -> Vec<TurnEvent> {
    stream.collect().await
}

fn text_of(events: &[TurnEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn finish_of(events: &[TurnEvent]) -> Option<FinishReason> {
    events.iter().rev().find_map(|e| match e {
        TurnEvent::Finished { reason } => Some(*reason),
        _ => None,
    })
}

fn errors_of(events: &[TurnEvent]) -> Vec<LlmError> {
    events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::Error { error } => Some(error.clone()),
            _ => None,
        })
        .collect()
}

fn drain(rx: &mut tokio::sync::broadcast::Receiver<BusEvent>) -> Vec<BusEvent> {
    let mut out = vec![];
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

#[tokio::test]
async fn a_plain_turn_streams_text_and_finishes() {
    let h = harness(
        Arc::new(ScriptedRuntime::new(vec![text_script("hello")])),
        false,
    );
    let events =
        collect(
            h.coordinator
                .run_turn("conv1", &fixed_agent(), "hi", CancellationToken::new()),
        )
        .await;
    assert!(matches!(events.first(), Some(TurnEvent::Started { .. })));
    assert_eq!(text_of(&events), "hello");
    assert_eq!(finish_of(&events), Some(FinishReason::Stop));
    assert_eq!(h.runtime.attempts(), 1);
}

#[tokio::test]
async fn the_bus_sees_the_turn_start_the_tokens_and_the_end() {
    let h = harness(
        Arc::new(ScriptedRuntime::new(vec![text_script("hey")])),
        false,
    );
    let mut rx = h.bus.subscribe();
    collect(
        h.coordinator
            .run_turn("c", &fixed_agent(), "hi", CancellationToken::new()),
    )
    .await;
    let evs = drain(&mut rx);
    assert!(matches!(evs.first(), Some(BusEvent::TurnStarted { .. })));
    assert!(evs
        .iter()
        .any(|e| matches!(e, BusEvent::TokenDelta { chars, .. } if *chars == 3)));
    assert!(matches!(evs.last(), Some(BusEvent::TurnEnded { .. })));
}

#[tokio::test]
async fn usage_is_aggregated_and_booked_in_the_budget() {
    let h = harness(
        Arc::new(ScriptedRuntime::new(vec![text_script("hello")])),
        false,
    );
    let events =
        collect(
            h.coordinator
                .run_turn("c", &fixed_agent(), "hi", CancellationToken::new()),
        )
        .await;
    let usage = events
        .iter()
        .rev()
        .find_map(|e| match e {
            TurnEvent::Usage { usage } => Some(usage.clone()),
            _ => None,
        })
        .expect("a usage event");
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 5);
    let spend = h.budget.spent_today("worker");
    assert_eq!(spend.turns, 1);
    assert!((spend.usd - 0.01).abs() < 1e-9);
}

#[tokio::test]
async fn the_conversation_is_appended_to_jsonl_and_replayed() {
    let h = harness(
        Arc::new(ScriptedRuntime::new(vec![text_script("one")])),
        true,
    );
    collect(
        h.coordinator
            .run_turn("conv-a", &fixed_agent(), "hi", CancellationToken::new()),
    )
    .await;
    let history = h.coordinator.history("conv-a").unwrap();
    // system prompt + user + assistant
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].role, Role::System);
    assert_eq!(history[1].role, Role::User);
    assert_eq!(history[2].role, Role::Assistant);
    let path = conversation_path(h.coordinator.dir().unwrap(), "conv-a").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 3);
}

#[tokio::test]
async fn the_second_turn_starts_from_the_stored_history() {
    let h = harness(
        Arc::new(ScriptedRuntime::new(vec![
            text_script("one"),
            text_script("two"),
        ])),
        true,
    );
    let agent = fixed_agent();
    collect(
        h.coordinator
            .run_turn("conv-b", &agent, "first", CancellationToken::new()),
    )
    .await;
    collect(
        h.coordinator
            .run_turn("conv-b", &agent, "second", CancellationToken::new()),
    )
    .await;
    let history = h.coordinator.history("conv-b").unwrap();
    assert_eq!(history.len(), 5, "system + 2 user + 2 assistant");
}

#[tokio::test]
async fn the_tool_loop_resolves_three_calls_then_answers() {
    let runtime = Arc::new(ScriptedRuntime::new(vec![
        tool_script("t1", "dl_add", r#"{"url":"a"}"#),
        tool_script("t2", "dl_add", r#"{"url":"b"}"#),
        tool_script("t3", "dl_add", r#"{"url":"c"}"#),
        text_script("all three queued"),
    ]));
    let h = harness(runtime.clone(), false);
    let agent = agent_with(
        ModelPolicy::Fixed {
            model: ModelRef {
                provider: ProviderId::new("fake"),
                model: "m".into(),
            },
        },
        vec![auto_grant("dl_add")],
        Budget {
            max_tool_calls_per_turn: 4,
            ..Budget::default()
        },
    );
    let mut rx = h.bus.subscribe();
    let events =
        collect(
            h.coordinator
                .run_turn("c", &agent, "queue three", CancellationToken::new()),
        )
        .await;
    assert_eq!(runtime.attempts(), 4, "three tool rounds plus the answer");
    assert_eq!(text_of(&events), "all three queued");
    assert_eq!(finish_of(&events), Some(FinishReason::Stop));
    let called = drain(&mut rx)
        .into_iter()
        .filter(|e| matches!(e, BusEvent::ToolCalled { ok: true, .. }))
        .count();
    assert_eq!(called, 3);
}

#[tokio::test]
async fn a_denied_tool_comes_back_as_an_error_result_not_a_dead_turn() {
    let runtime = Arc::new(ScriptedRuntime::new(vec![
        tool_script("t1", "dl_add", "{}"),
        text_script("could not run it"),
    ]));
    let h = harness(runtime.clone(), false);
    // No grant at all: the broker denies.
    let agent = agent_with(
        ModelPolicy::Fixed {
            model: ModelRef {
                provider: ProviderId::new("fake"),
                model: "m".into(),
            },
        },
        vec![],
        Budget::default(),
    );
    let events = collect(
        h.coordinator
            .run_turn("c", &agent, "go", CancellationToken::new()),
    )
    .await;
    assert_eq!(finish_of(&events), Some(FinishReason::Stop));
    assert_eq!(runtime.attempts(), 2);
    assert!(
        errors_of(&events).is_empty(),
        "the denial is a tool result, not a turn error"
    );
}

#[tokio::test]
async fn more_tool_calls_than_the_budget_allows_stops_the_turn() {
    let runtime = Arc::new(ScriptedRuntime::new(vec![
        tool_script("t1", "dl_add", "{}"),
        tool_script("t2", "dl_add", "{}"),
        tool_script("t3", "dl_add", "{}"),
    ]));
    let h = harness(runtime.clone(), false);
    let agent = agent_with(
        ModelPolicy::Fixed {
            model: ModelRef {
                provider: ProviderId::new("fake"),
                model: "m".into(),
            },
        },
        vec![auto_grant("dl_add")],
        Budget {
            max_tool_calls_per_turn: 2,
            ..Budget::default()
        },
    );
    let events = collect(
        h.coordinator
            .run_turn("c", &agent, "go", CancellationToken::new()),
    )
    .await;
    let errs = errors_of(&events);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].code, ERR_LLM_TOOL_LIMIT);
    assert_eq!(finish_of(&events), Some(FinishReason::Other));
}

#[tokio::test]
async fn cancelling_in_the_middle_stops_the_stream() {
    let runtime = Arc::new(
        ScriptedRuntime::new(vec![vec![
            TurnEvent::Started {
                request_id: "r".into(),
            },
            TurnEvent::TextDelta { text: "a".into() },
            TurnEvent::TextDelta { text: "b".into() },
            TurnEvent::TextDelta { text: "c".into() },
            TurnEvent::TextDelta { text: "d".into() },
            TurnEvent::Finished {
                reason: FinishReason::Stop,
            },
        ]])
        .with_delay(Duration::from_millis(20)),
    );
    let h = harness(runtime, false);
    let cancel = CancellationToken::new();
    let mut stream = h
        .coordinator
        .run_turn("c", &fixed_agent(), "hi", cancel.clone());
    let mut seen = Vec::new();
    while let Some(ev) = stream.next().await {
        let is_delta = matches!(ev, TurnEvent::TextDelta { .. });
        seen.push(ev);
        if is_delta
            && seen
                .iter()
                .filter(|e| matches!(e, TurnEvent::TextDelta { .. }))
                .count()
                == 2
        {
            cancel.cancel();
        }
    }
    assert_eq!(finish_of(&seen), Some(FinishReason::Cancelled));
    let deltas = seen
        .iter()
        .filter(|e| matches!(e, TurnEvent::TextDelta { .. }))
        .count();
    assert!(
        deltas < 4,
        "cancelled before the script ended, got {deltas} deltas"
    );
    assert!(
        !seen.iter().any(|e| matches!(e, TurnEvent::Usage { .. })),
        "a cancelled turn does not report usage"
    );
}

#[tokio::test]
async fn the_budget_cuts_before_the_provider_is_called() {
    let runtime = Arc::new(ScriptedRuntime::new(vec![text_script("never")]));
    let h = harness(runtime.clone(), false);
    let agent = agent_with(
        ModelPolicy::Fixed {
            model: ModelRef {
                provider: ProviderId::new("fake"),
                model: "m".into(),
            },
        },
        vec![],
        Budget {
            usd_per_day: Some(0.10),
            ..Budget::default()
        },
    );
    h.budget.record("worker", Some(0.50), 0, 0);
    let mut rx = h.bus.subscribe();
    let events = collect(
        h.coordinator
            .run_turn("c", &agent, "hi", CancellationToken::new()),
    )
    .await;
    assert_eq!(runtime.attempts(), 0, "the model was never called");
    let errs = errors_of(&events);
    assert_eq!(errs[0].code, ERR_LLM_BUDGET);
    assert_eq!(finish_of(&events), Some(FinishReason::Other));
    assert!(drain(&mut rx)
        .iter()
        .any(|e| matches!(e, BusEvent::BudgetHit { .. })));
}

#[tokio::test]
async fn the_token_ceiling_cuts_a_turn_that_is_too_big() {
    let runtime = Arc::new(ScriptedRuntime::new(vec![text_script("never")]));
    let h = harness(runtime.clone(), false);
    let agent = agent_with(
        ModelPolicy::Fixed {
            model: ModelRef {
                provider: ProviderId::new("fake"),
                model: "m".into(),
            },
        },
        vec![],
        Budget {
            tokens_per_turn: Some(4),
            ..Budget::default()
        },
    );
    let long = "x".repeat(4_000);
    let events = collect(
        h.coordinator
            .run_turn("c", &agent, &long, CancellationToken::new()),
    )
    .await;
    assert_eq!(runtime.attempts(), 0);
    assert_eq!(errors_of(&events)[0].code, ERR_LLM_BUDGET);
}

#[tokio::test]
async fn a_rate_limit_reroutes_to_the_fallback_without_losing_the_turn() {
    let runtime = Arc::new(ScriptedRuntime::new(vec![
        rate_limited(),
        text_script("second wins"),
    ]));
    let h = harness(runtime.clone(), false);
    let agent = agent_with(
        ModelPolicy::Route {
            chain: vec![
                native("anthropic", "sonnet"),
                native("openrouter", "sonnet"),
            ],
        },
        vec![],
        Budget::default(),
    );
    let mut rx = h.bus.subscribe();
    let events = collect(
        h.coordinator
            .run_turn("c", &agent, "hi", CancellationToken::new()),
    )
    .await;
    assert_eq!(text_of(&events), "second wins");
    assert!(errors_of(&events).is_empty());
    assert_eq!(
        h.runtime.seen(),
        vec![
            "anthropic:sonnet".to_string(),
            "openrouter:sonnet".to_string()
        ]
    );
    let rerouted: Vec<BusEvent> = drain(&mut rx)
        .into_iter()
        .filter(|e| matches!(e, BusEvent::Rerouted { .. }))
        .collect();
    assert_eq!(rerouted.len(), 1);
    match &rerouted[0] {
        BusEvent::Rerouted { from, to, why, .. } => {
            assert_eq!(from, "native:anthropic:sonnet");
            assert_eq!(to, "native:openrouter:sonnet");
            assert_eq!(why, ERR_LLM_RATE);
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn a_chain_of_four_with_three_failures_still_answers() {
    let runtime = Arc::new(ScriptedRuntime::new(vec![
        rate_limited(),
        rate_limited(),
        rate_limited(),
        text_script("the fourth one answered"),
    ]));
    let h = harness(runtime.clone(), false);
    let agent = agent_with(
        ModelPolicy::Route {
            chain: vec![
                native("p1", "m"),
                native("p2", "m"),
                native("p3", "m"),
                native("p4", "m"),
            ],
        },
        vec![],
        Budget::default(),
    );
    let mut rx = h.bus.subscribe();
    let events = collect(
        h.coordinator
            .run_turn("c", &agent, "hi", CancellationToken::new()),
    )
    .await;
    assert_eq!(text_of(&events), "the fourth one answered");
    assert_eq!(
        h.runtime.seen(),
        vec!["p1:m", "p2:m", "p3:m", "p4:m"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        drain(&mut rx)
            .into_iter()
            .filter(|e| matches!(e, BusEvent::Rerouted { .. }))
            .count(),
        3
    );
}

#[tokio::test]
async fn when_the_whole_chain_is_rate_limited_the_error_reaches_the_user() {
    let runtime = Arc::new(ScriptedRuntime::new(vec![rate_limited(), rate_limited()]));
    let h = harness(runtime.clone(), false);
    let agent = agent_with(
        ModelPolicy::Route {
            chain: vec![native("p1", "m"), native("p2", "m")],
        },
        vec![],
        Budget::default(),
    );
    let events = collect(
        h.coordinator
            .run_turn("c", &agent, "hi", CancellationToken::new()),
    )
    .await;
    let errs = errors_of(&events);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].code, ERR_LLM_RATE);
    assert_eq!(h.runtime.attempts(), 2);
}

#[tokio::test]
async fn a_capacity_source_that_blocks_the_first_candidate_starts_at_the_second() {
    let runtime = Arc::new(ScriptedRuntime::new(vec![text_script("from the fallback")]));
    let source = StaticCapacity::new().set(
        "native:p1:m",
        Capacity {
            last_error: Some(CapacityError {
                code: ERR_CLI_RATE.into(),
                seconds_ago: 5,
            }),
            ..Capacity::default()
        },
    );
    let router = Arc::new(Router::new(Arc::new(source)).with_config(RouterConfig::default()));
    let h = harness_with(runtime.clone(), false, vec![spec("dl_add")], Some(router));
    let agent = agent_with(
        ModelPolicy::Route {
            chain: vec![native("p1", "m"), native("p2", "m")],
        },
        vec![],
        Budget::default(),
    );
    collect(
        h.coordinator
            .run_turn("c", &agent, "hi", CancellationToken::new()),
    )
    .await;
    assert_eq!(runtime.seen(), vec!["p2:m".to_string()]);
}

#[tokio::test]
async fn a_non_rate_error_is_not_rerouted() {
    let runtime = Arc::new(ScriptedRuntime::new(vec![
        vec![TurnEvent::Error {
            error: LlmError::new(crate::core::llm::error::ERR_LLM_AUTH, "bad key"),
        }],
        text_script("never reached"),
    ]));
    let h = harness(runtime.clone(), false);
    let agent = agent_with(
        ModelPolicy::Route {
            chain: vec![native("p1", "m"), native("p2", "m")],
        },
        vec![],
        Budget::default(),
    );
    let events = collect(
        h.coordinator
            .run_turn("c", &agent, "hi", CancellationToken::new()),
    )
    .await;
    assert_eq!(
        errors_of(&events)[0].code,
        crate::core::llm::error::ERR_LLM_AUTH
    );
    assert_eq!(runtime.attempts(), 1);
}

#[tokio::test]
async fn an_empty_chain_fails_the_turn_with_a_router_code() {
    let h = harness(Arc::new(ScriptedRuntime::new(vec![])), false);
    let agent = agent_with(
        ModelPolicy::Route { chain: vec![] },
        vec![],
        Budget::default(),
    );
    let events = collect(
        h.coordinator
            .run_turn("c", &agent, "hi", CancellationToken::new()),
    )
    .await;
    assert_eq!(
        errors_of(&events)[0].code,
        crate::core::llm::router::ERR_LLM_NO_CANDIDATE
    );
}

#[tokio::test]
async fn handoff_writes_a_summary_the_next_agent_reads() {
    let h = harness(
        Arc::new(ScriptedRuntime::new(vec![text_script("ok")])),
        true,
    );
    let from = fixed_agent();
    let mut to = fixed_agent();
    to.id = "advisor".into();
    to.name = "Advisor".into();
    let msg = h
        .coordinator
        .handoff("conv-h", &from, &to, "downloaded 3 files, 1 failed")
        .unwrap();
    assert_eq!(msg.role, Role::System);
    let history = h.coordinator.history("conv-h").unwrap();
    assert_eq!(history.len(), 1);
    match &history[0].parts[0] {
        ContentPart::Text { text } => {
            assert!(text.contains("Advisor"));
            assert!(text.contains("1 failed"));
        }
        other => panic!("unexpected part {other:?}"),
    }
}

#[test]
fn unsafe_conversation_ids_are_refused() {
    assert!(is_safe_id("abc-123_DEF"));
    assert!(!is_safe_id(""));
    assert!(!is_safe_id("../etc/passwd"));
    assert!(!is_safe_id("a/b"));
    let err = conversation_path(Path::new("/tmp"), "../x").unwrap_err();
    assert_eq!(err.code, ERR_LLM_CONVERSATION);
}

#[test]
fn the_token_estimate_is_four_characters_per_token() {
    let messages = vec![Message::text(Role::User, "a".repeat(400))];
    assert_eq!(estimate_tokens(&messages), 100);
    assert_eq!(estimate_tokens(&[]), 0);
}

#[test]
fn a_truncated_jsonl_line_is_skipped_not_fatal() {
    let dir = std::env::temp_dir().join(format!("omniget-conv-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    append_message(&dir, "c", Some("a"), &Message::text(Role::User, "one"));
    let path = conversation_path(&dir, "c").unwrap();
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("{\"ts\":\"broken\"\n");
    std::fs::write(&path, text).unwrap();
    let loaded = load_conversation(&dir, "c").unwrap();
    assert_eq!(loaded.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn coordinator_overhead_per_turn_stays_under_five_milliseconds() {
    let n = 50;
    let scripts: Vec<Vec<TurnEvent>> = (0..n).map(|_| text_script("hello")).collect();
    let h = harness(Arc::new(ScriptedRuntime::new(scripts)), false);
    let agent = fixed_agent();
    // Warm up the task spawner and the channel.
    collect(
        h.coordinator
            .run_turn("c", &agent, "hi", CancellationToken::new()),
    )
    .await;
    let start = std::time::Instant::now();
    for _ in 1..n {
        collect(
            h.coordinator
                .run_turn("c", &agent, "hi", CancellationToken::new()),
        )
        .await;
    }
    let per = start.elapsed() / (n - 1) as u32;
    println!("coordinator overhead per turn: {per:?}");
    assert!(
        per < Duration::from_millis(5),
        "overhead was {per:?} per turn"
    );
}

#[tokio::test]
async fn the_callers_request_id_reaches_started_and_tool_ask() {
    let runtime = Arc::new(ScriptedRuntime::new(vec![
        tool_script("t1", "dl_add", "{}"),
        text_script("done"),
    ]));
    let h = harness(runtime, false);
    let agent = agent_with(
        ModelPolicy::Fixed {
            model: ModelRef {
                provider: ProviderId::new("fake"),
                model: "m".into(),
            },
        },
        vec![ToolGrant {
            source: ToolSource::Internal {
                name: "dl_add".into(),
            },
            mode: GrantMode::Ask,
        }],
        Budget::default(),
    );
    let mut rx = h.bus.subscribe();
    let coordinator = h.coordinator.clone();
    let broker = coordinator.broker().clone();
    let task = tokio::spawn(async move {
        collect(coordinator.run_turn_with_id(
            "turn-42".into(),
            "c",
            &agent,
            "go",
            CancellationToken::new(),
        ))
        .await
    });
    // Answer the ask as the UI would, keyed by the tool call id.
    let mut asked = None;
    while asked.is_none() {
        if let Ok(BusEvent::ToolAsk {
            request_id,
            tool_call_id,
            ..
        }) = rx.recv().await
        {
            assert_eq!(
                request_id, "turn-42",
                "the caller's turn id, not a fresh uuid"
            );
            asked = Some(tool_call_id);
        }
    }
    let tool_call_id = asked.unwrap();
    while !broker.answer(&tool_call_id, true) {
        tokio::task::yield_now().await;
    }
    let events = task.await.unwrap();
    let started = events
        .iter()
        .find_map(|e| match e {
            TurnEvent::Started { request_id } => Some(request_id.clone()),
            _ => None,
        })
        .expect("a started event");
    assert_eq!(started, "turn-42");
    assert_eq!(text_of(&events), "done");
}
