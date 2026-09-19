//! The agent brain: observe → memorise → reflect → plan, all of it outside
//! the world tick. Owned by f7-world-brain.
//!
//! The shape is smallville's (`estudos/72` §3.2 — the concept, not the Java
//! server): what happens near an agent becomes an observation, observations
//! become memories with an embedding, a day of memories becomes a handful of
//! reflections, and reflections become the next day's agenda. What is ours is
//! the budget around it (plan §3 Fase 7):
//!
//! * **zero model calls inside the tick.** [`Brain::tick_hint`] is a pure
//!   function of (minute of the game day, energy, last decision). The world
//!   is alive whether or not a token is ever spent.
//!   ,
//! * **one call in flight per agent.** [`Brain::think`] takes a flag; a second
//!   caller gets `ERR_BRAIN_BUSY` instead of a second turn.
//! * **an idle agent is free.** No observation for
//!   [`BrainConfig::idle_game_minutes`] means [`Brain::should_think`] is
//!   false and nothing is called.
//! * **energy is quota** (plan §9.1). Zero energy means zero model calls, no
//!   exception, until the window renews and the bridge sends
//!   [`WorldEvent::Dawn`].
//!
//! ## The mirror types
//!
//! `omniget-core` does not depend on `omniget-world` yet (the handoff asks
//! for the dependency under `cargo.deps`). [`Decision`], [`WorldEvent`],
//! [`Tile`], [`EntId`] and [`ObjectId`] here are field-for-field mirrors of
//! the crate's, with the same variant order and the same `tag()` numbers, so
//! the conversion the bridge writes is a `match` with no decisions in it.
//! `WorldEvent::Dawn` is the one addition: the world has no such event (tags
//! stop at 12), it is synthesised by the bridge when an account's window
//! renews. Once the dependency lands, these become `pub use
//! omniget_world::{…}` and the `From` impls go away.

pub mod memory;
pub mod observe;
pub mod plan;
pub mod prompt;
pub mod reflect;
pub mod schedule;

use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use self::memory::{embed_all, Embedder, InMemoryStore, Memory, MemoryKind, MemoryStore};
use self::observe::{Cast, Observation, ObserveQueue};
use self::plan::ObjectIndex;
use self::schedule::{
    day_of, hint, in_sleep_window, minute_of_day, Places, Schedule, TICKS_PER_GAME_MINUTE,
};
use super::agent::AgentDef;
use super::coordinator::Coordinator;
use super::error::LlmError;
use super::router::Capacity;
use super::types::{FinishReason, TurnEvent};
use crate::core::omni::bus::BusEvent;

/// A second `think` was asked for while one was in flight.
pub const ERR_BRAIN_BUSY: &str = "ERR_BRAIN_BUSY";
/// The account backing this agent has no window left. No model is called.
pub const ERR_BRAIN_NO_ENERGY: &str = "ERR_BRAIN_NO_ENERGY";
/// The embedder failed or answered with the wrong number of vectors.
pub const ERR_BRAIN_EMBED: &str = "ERR_BRAIN_EMBED";
/// The model's plan could not be read at all.
pub const ERR_BRAIN_PLAN: &str = "ERR_BRAIN_PLAN";
/// The memory store failed.
pub const ERR_BRAIN_STORE: &str = "ERR_BRAIN_STORE";
/// The model call itself failed; the `LlmError` code is in the message.
pub const ERR_BRAIN_MODEL: &str = "ERR_BRAIN_MODEL";

/// A brain error with a stable code the UI can map, in the mould of
/// `LlmError`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrainError {
    pub code: Cow<'static, str>,
    pub message: String,
}

impl BrainError {
    pub fn new(code: &'static str, message: impl Into<String>) -> BrainError {
        BrainError {
            code: Cow::Borrowed(code),
            message: message.into(),
        }
    }
    pub fn busy() -> BrainError {
        BrainError::new(ERR_BRAIN_BUSY, "a turn is already in flight for this agent")
    }
    pub fn no_energy() -> BrainError {
        BrainError::new(ERR_BRAIN_NO_ENERGY, "no quota window left; sleeping")
    }
    pub fn embed(message: impl Into<String>) -> BrainError {
        BrainError::new(ERR_BRAIN_EMBED, message)
    }
    pub fn plan(message: impl Into<String>) -> BrainError {
        BrainError::new(ERR_BRAIN_PLAN, message)
    }
    pub fn store(message: impl Into<String>) -> BrainError {
        BrainError::new(ERR_BRAIN_STORE, message)
    }
    pub fn no_memory(id: i64) -> BrainError {
        BrainError::new(ERR_BRAIN_STORE, format!("no memory with id {id}"))
    }
}

impl From<LlmError> for BrainError {
    fn from(err: LlmError) -> BrainError {
        BrainError::new(ERR_BRAIN_MODEL, format!("{}: {}", err.code, err.message))
    }
}

impl std::fmt::Display for BrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for BrainError {}

// ---------------------------------------------------------------------------
// Mirrors of `omniget_world`. See the module doc.
// ---------------------------------------------------------------------------

/// Mirror of `omniget_world::EntId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct EntId(pub u32);

/// Mirror of `omniget_world::ObjectId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct ObjectId(pub u32);

/// Mirror of `omniget_world::Tile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tile {
    pub x: i32,
    pub y: i32,
}

impl Tile {
    pub const fn new(x: i32, y: i32) -> Tile {
        Tile { x, y }
    }
}

/// Mirror of `omniget_world::Decision`, same variants in the same order, so
/// `tag()` matches the world's byte for byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    GoTo(Tile),
    Sit(ObjectId),
    Sleep,
    Work(ObjectId),
    /// The world cuts this at 200 bytes; `prompt::clamp_say` cuts it first.
    Say(String),
    Wave(EntId),
    Idle,
}

impl Decision {
    pub const fn tag(&self) -> u8 {
        match self {
            Decision::GoTo(_) => 0,
            Decision::Sit(_) => 1,
            Decision::Sleep => 2,
            Decision::Work(_) => 3,
            Decision::Say(_) => 4,
            Decision::Wave(_) => 5,
            Decision::Idle => 6,
        }
    }
}

/// Mirror of `omniget_world::WorldEvent`, plus [`WorldEvent::Dawn`], which the
/// bridge synthesises when a quota window renews (plan §9.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldEvent {
    Arrived {
        ent: EntId,
        at: Tile,
    },
    StartedAnim {
        ent: EntId,
        anim: u8,
        dir: u8,
    },
    Said {
        ent: EntId,
        text: String,
    },
    Interacted {
        ent: EntId,
        object: ObjectId,
    },
    Slept {
        ent: EntId,
    },
    Woke {
        ent: EntId,
    },
    Yawned {
        ent: EntId,
    },
    Spawned {
        ent: EntId,
        at: Tile,
    },
    Despawned {
        ent: EntId,
    },
    ObjectPlaced {
        object: ObjectId,
        at: Tile,
    },
    ObjectRemoved {
        object: ObjectId,
    },
    CaughtUp {
        ticks: u64,
    },
    Rejected {
        ent: EntId,
        code: String,
    },
    /// Not a world event: the quota window renewed and the house lights up.
    Dawn,
}

impl WorldEvent {
    /// The world's wire tag, or `None` for [`WorldEvent::Dawn`], which never
    /// travels on the world wire.
    pub const fn tag(&self) -> Option<u8> {
        Some(match self {
            WorldEvent::Arrived { .. } => 0,
            WorldEvent::StartedAnim { .. } => 1,
            WorldEvent::Said { .. } => 2,
            WorldEvent::Interacted { .. } => 3,
            WorldEvent::Slept { .. } => 4,
            WorldEvent::Woke { .. } => 5,
            WorldEvent::Yawned { .. } => 6,
            WorldEvent::Spawned { .. } => 7,
            WorldEvent::Despawned { .. } => 8,
            WorldEvent::ObjectPlaced { .. } => 9,
            WorldEvent::ObjectRemoved { .. } => 10,
            WorldEvent::CaughtUp { .. } => 11,
            WorldEvent::Rejected { .. } => 12,
            WorldEvent::Dawn => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Thinking
// ---------------------------------------------------------------------------

/// One question, one answer. The brain never touches a provider, a stream or
/// a conversation id: [`CoordinatorThinker`] does that, and the tests use a
/// scripted one.
#[async_trait]
pub trait Thinker: Send + Sync {
    async fn ask(&self, prompt: &str) -> Result<String, BrainError>;
}

/// A [`Thinker`] over the F2 `Coordinator`: one turn per question, text
/// deltas collected, tools and budget already handled upstream.
pub struct CoordinatorThinker {
    coordinator: Arc<Coordinator>,
    agent: AgentDef,
    conversation_id: String,
}

impl std::fmt::Debug for CoordinatorThinker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoordinatorThinker")
            .field("agent", &self.agent.id)
            .field("conversation_id", &self.conversation_id)
            .finish()
    }
}

impl CoordinatorThinker {
    /// The conversation id is the agent's brain log: one per agent, so the
    /// transcript on disk reads as that agent's inner life.
    pub fn new(coordinator: Arc<Coordinator>, agent: AgentDef) -> CoordinatorThinker {
        let conversation_id = format!("brain-{}", agent.id);
        CoordinatorThinker {
            coordinator,
            agent,
            conversation_id,
        }
    }

    pub fn with_conversation(mut self, id: impl Into<String>) -> CoordinatorThinker {
        self.conversation_id = id.into();
        self
    }
}

#[async_trait]
impl Thinker for CoordinatorThinker {
    async fn ask(&self, prompt: &str) -> Result<String, BrainError> {
        let mut stream = self.coordinator.run_turn(
            &self.conversation_id,
            &self.agent,
            prompt,
            CancellationToken::new(),
        );
        let mut answer = String::new();
        while let Some(event) = stream.next().await {
            match event {
                TurnEvent::TextDelta { text } => answer.push_str(&text),
                TurnEvent::Error { error } => return Err(error.into()),
                TurnEvent::Finished { reason } => {
                    if reason == FinishReason::Cancelled {
                        return Err(BrainError::new(ERR_BRAIN_MODEL, "turn cancelled"));
                    }
                    break;
                }
                _ => {}
            }
        }
        Ok(answer)
    }
}

// ---------------------------------------------------------------------------
// The brain
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainConfig {
    /// Game minutes without a new observation after which the agent is idle
    /// and costs nothing.
    pub idle_game_minutes: u64,
    /// Memories handed to a planning prompt.
    pub retrieve_k: usize,
    /// Memories kept per agent; the oldest beyond this are pruned after a
    /// reflection has consolidated them.
    pub keep_memories: usize,
    /// Observations buffered for the daily reflection.
    pub day_log_cap: usize,
}

impl Default for BrainConfig {
    fn default() -> Self {
        BrainConfig {
            idle_game_minutes: 2,
            retrieve_k: 8,
            keep_memories: 500,
            day_log_cap: 200,
        }
    }
}

/// What one `think` did, for the log, the HUD and the tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThoughtReport {
    pub memories_stored: usize,
    pub reflections: usize,
    /// Model calls this `think` made: 0 or 1, never more.
    pub model_calls: u32,
    pub planned: bool,
    /// The plan could not be read and the default day was used.
    pub improvised: bool,
    pub decisions: Vec<Decision>,
}

#[derive(Debug)]
struct BrainState {
    tick: u64,
    energy: u8,
    schedule: Schedule,
    queue: ObserveQueue,
    day_log: VecDeque<Observation>,
    last_decision: Option<Decision>,
    last_observation_tick: u64,
    last_plan_day: u64,
    model_calls: u32,
}

/// One agent's mind. Cheap to build, cheap to keep: a sleeping agent holds a
/// queue, a schedule and two counters.
pub struct Brain {
    pub agent_id: String,
    cast: Cast,
    places: Places,
    objects: ObjectIndex,
    store: Arc<dyn MemoryStore>,
    embedder: Arc<dyn Embedder>,
    in_flight: AtomicBool,
    state: Mutex<BrainState>,
    config: BrainConfig,
}

impl std::fmt::Debug for Brain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let st = self.lock();
        f.debug_struct("Brain")
            .field("agent_id", &self.agent_id)
            .field("ent", &self.cast.me)
            .field("tick", &st.tick)
            .field("energy", &st.energy)
            .field("queue", &st.queue.len())
            .field("model_calls", &st.model_calls)
            .finish()
    }
}

/// Releases the in-flight flag however the turn ends, panic included.
struct InFlight<'a>(&'a AtomicBool);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Brain {
    pub fn new(
        agent_id: impl Into<String>,
        cast: Cast,
        places: Places,
        store: Arc<dyn MemoryStore>,
        embedder: Arc<dyn Embedder>,
    ) -> Brain {
        let objects = plan::index_from_places(&places);
        Brain {
            agent_id: agent_id.into(),
            cast,
            places,
            objects,
            store,
            embedder,
            in_flight: AtomicBool::new(false),
            state: Mutex::new(BrainState {
                tick: 0,
                energy: schedule::ENERGY_FULL,
                schedule: schedule::default_schedule(&places),
                queue: ObserveQueue::new(),
                day_log: VecDeque::new(),
                last_decision: None,
                last_observation_tick: 0,
                last_plan_day: 0,
                model_calls: 0,
            }),
            config: BrainConfig::default(),
        }
    }

    /// A brain with nothing wired: in-memory store, deterministic fake
    /// embedder. What a first run uses before the model is downloaded.
    pub fn detached(agent_id: impl Into<String>, ent: EntId, name: impl Into<String>) -> Brain {
        let name = name.into();
        Brain::new(
            agent_id,
            Cast::new(ent, name),
            Places::default(),
            Arc::new(InMemoryStore::new()),
            Arc::new(memory::FakeEmbedder),
        )
    }

    pub fn with_config(mut self, config: BrainConfig) -> Brain {
        self.config = config;
        self
    }

    /// Every object in the house the agent may name in a plan.
    pub fn with_objects(mut self, objects: ObjectIndex) -> Brain {
        self.objects.extend(objects);
        self
    }

    pub fn with_schedule(self, schedule: Schedule) -> Brain {
        self.lock().schedule = schedule;
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BrainState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn ent(&self) -> EntId {
        self.cast.me
    }

    pub fn config(&self) -> &BrainConfig {
        &self.config
    }

    pub fn tick(&self) -> u64 {
        self.lock().tick
    }

    pub fn energy(&self) -> u8 {
        self.lock().energy
    }

    pub fn schedule(&self) -> Schedule {
        self.lock().schedule.clone()
    }

    /// Model calls this brain has made since it was built. The tick budget is
    /// "this number does not move while only `tick_hint` is called".
    pub fn model_calls(&self) -> u32 {
        self.lock().model_calls
    }

    pub fn queue_len(&self) -> usize {
        self.lock().queue.len()
    }

    pub fn memory_count(&self) -> usize {
        self.store.count(&self.agent_id).unwrap_or(0)
    }

    pub fn is_thinking(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// The bridge owns the clock; the brain is told what tick it is.
    pub fn set_tick(&self, tick: u64) {
        self.lock().tick = tick;
    }

    /// Quota becomes energy (plan §9.1). Returns what the agent does about it
    /// right now: a yawn on the way down, nothing otherwise.
    pub fn set_energy(&self, energy: u8) -> Vec<Decision> {
        let mut st = self.lock();
        let before = st.energy;
        st.energy = energy;
        let tick = st.tick;
        drop(st);
        if schedule::crossed_into_tired(before, energy) {
            vec![Decision::Say(prompt::clamp_say(prompt::yawn_line(tick)))]
        } else {
            Vec::new()
        }
    }

    /// The same, straight from the router's capacity view.
    pub fn set_capacity(
        &self,
        capacity: &Capacity,
        budget_today_usd: Option<f64>,
    ) -> Vec<Decision> {
        self.set_energy(schedule::energy_from_capacity(capacity, budget_today_usd))
    }

    /// World events become observations. Returns how many were kept.
    pub fn observe(&self, events: &[WorldEvent]) -> usize {
        let mut st = self.lock();
        let tick = st.tick;
        let mut kept = 0;
        for event in events {
            if event == &WorldEvent::Dawn {
                st.last_decision = None;
            }
            if let Some(observation) = observe::observe_event(&self.cast, event, tick) {
                if st.queue.push(observation.clone()) {
                    push_day_log(&mut st, observation, self.config.day_log_cap);
                    kept += 1;
                }
            }
        }
        if kept > 0 {
            st.last_observation_tick = tick;
        }
        kept
    }

    /// A bus event is both something to remember and, for this agent's own
    /// work, something to do in the house: plan §9.1's diegetic work, at zero
    /// token cost.
    pub fn observe_bus(&self, event: &BusEvent) -> Vec<Decision> {
        let mut st = self.lock();
        let tick = st.tick;
        if let Some(observation) = observe::observe_bus(&self.agent_id, event, tick) {
            if st.queue.push(observation.clone()) {
                push_day_log(&mut st, observation, self.config.day_log_cap);
                st.last_observation_tick = tick;
            }
        }
        let decision = observe::work_decision(&self.agent_id, event, &self.places);
        match decision {
            Some(d) if st.last_decision.as_ref() != Some(&d) => {
                st.last_decision = Some(d.clone());
                vec![d]
            }
            _ => Vec::new(),
        }
    }

    /// The agenda's answer for this tick. No model, no allocation unless the
    /// decision actually changed, and `None` when the agent is already doing
    /// the right thing.
    pub fn tick_hint(&self, now_tick: u64) -> Option<Decision> {
        let mut st = self.lock();
        st.tick = now_tick;
        let minute = minute_of_day(now_tick);
        let decision = hint(&st.schedule, minute, st.energy, st.last_decision.as_ref())?;
        st.last_decision = Some(decision.clone());
        Some(decision)
    }

    /// Is a model call worth making right now? False for an agent that is
    /// asleep, out of energy, already thinking, or that has seen nothing for
    /// [`BrainConfig::idle_game_minutes`].
    pub fn should_think(&self, now_tick: u64) -> bool {
        if self.is_thinking() {
            return false;
        }
        let st = self.lock();
        if st.energy == 0 {
            return false;
        }
        let minute = minute_of_day(now_tick);
        if in_sleep_window(
            minute,
            schedule::effective_bedtime(st.schedule.bedtime(), st.energy),
            st.schedule.wake(),
        ) {
            return false;
        }
        let day = day_of(now_tick);
        let plan_due = day > st.last_plan_day;
        let reflect_due = reflect::should_reflect(
            day,
            self.store.last_reflection_day(&self.agent_id).unwrap_or(0),
            st.day_log.len(),
        );
        if !plan_due && !reflect_due {
            return false;
        }
        let idle_ticks = now_tick.saturating_sub(st.last_observation_tick);
        let idle_limit = self.config.idle_game_minutes * TICKS_PER_GAME_MINUTE;
        // A brand new brain has never observed anything and is allowed its
        // first plan; after that, silence means sleep.
        st.last_observation_tick == 0 || idle_ticks <= idle_limit || reflect_due
    }

    /// The expensive half: at most one model call, and only when
    /// [`should_think`](Self::should_think) already said yes.
    ///
    /// Order of business: drain the queue into memory (embedded in batches,
    /// no model call), then either reflect on yesterday **or** plan today —
    /// never both in one call, so one `think` is one turn at most.
    pub async fn think(&self, thinker: &dyn Thinker) -> Result<ThoughtReport, BrainError> {
        if self
            .in_flight
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(BrainError::busy());
        }
        let _guard = InFlight(&self.in_flight);

        let (tick, energy, day, schedule) = {
            let st = self.lock();
            (st.tick, st.energy, day_of(st.tick), st.schedule.clone())
        };
        if energy == 0 {
            return Err(BrainError::no_energy());
        }

        let mut report = ThoughtReport {
            memories_stored: self.memorise(tick).await?,
            ..ThoughtReport::default()
        };

        let last_reflection_day = self
            .store
            .last_reflection_day(&self.agent_id)
            .unwrap_or_default();
        let day_log: Vec<Observation> = self.lock().day_log.iter().cloned().collect();

        if reflect::should_reflect(day, last_reflection_day, day_log.len()) {
            let picked = reflect::select_for_reflection(&day_log, reflect::REFLECT_WINDOW);
            let answer = thinker
                .ask(&prompt::reflect_prompt(
                    &self.cast.my_name,
                    day.saturating_sub(1),
                    &picked,
                ))
                .await?;
            report.model_calls = 1;
            let parsed = reflect::parse_reflections(&answer);
            for memory in reflect::into_memories(&self.agent_id, &parsed, tick) {
                self.store.insert(memory)?;
            }
            report.reflections = parsed.len();
            self.store.set_last_reflection_day(&self.agent_id, day).ok();
            self.store
                .prune(&self.agent_id, self.config.keep_memories)?;
            let mut st = self.lock();
            st.day_log.clear();
            st.model_calls += 1;
            return Ok(report);
        }

        if day <= self.lock().last_plan_day {
            return Ok(report);
        }

        let query = self
            .embedder
            .embed_batch(&["what should I do today"])
            .await?
            .into_iter()
            .next()
            .unwrap_or_default();
        let memories = memory::retrieve(
            self.store.as_ref(),
            &self.agent_id,
            &query,
            tick,
            self.config.retrieve_k,
        )?;
        let objects: Vec<String> = self.objects.keys().cloned().collect();
        let answer = thinker
            .ask(&prompt::plan_prompt(
                &self.cast.my_name,
                day,
                schedule::energy_pct(energy),
                &memories,
                &objects,
                &schedule,
            ))
            .await?;
        report.model_calls = 1;
        let (new_schedule, used) = plan::schedule_from_answer(&answer, &self.objects, &self.places);
        report.planned = true;
        report.improvised = !used;

        let mut st = self.lock();
        st.model_calls += 1;
        st.last_plan_day = day;
        st.schedule = new_schedule;
        if let Some(entry) = st.schedule.current(minute_of_day(tick)) {
            let line = prompt::clamp_say(&format!("Today: {}", entry.label));
            report.decisions.push(Decision::Say(line));
        }
        if let Some(decision) = hint(
            &st.schedule,
            minute_of_day(tick),
            st.energy,
            st.last_decision.as_ref(),
        ) {
            st.last_decision = Some(decision.clone());
            report.decisions.push(decision);
        }
        Ok(report)
    }

    /// Queue → memories, embedded in batches of 64. No model call: the
    /// importance comes from the observation, not from a rating turn.
    async fn memorise(&self, tick: u64) -> Result<usize, BrainError> {
        let observations = self.lock().queue.drain();
        if observations.is_empty() {
            return Ok(0);
        }
        let texts: Vec<String> = observations.iter().map(|o| o.text.clone()).collect();
        let vectors = embed_all(self.embedder.as_ref(), &texts).await?;
        for (observation, embedding) in observations.iter().zip(vectors) {
            let mut memory = Memory::new(
                &self.agent_id,
                MemoryKind::Observation,
                &observation.text,
                observation.importance,
                observation.tick.min(tick),
            );
            memory.embedding = Some(embedding);
            self.store.insert(memory)?;
        }
        Ok(observations.len())
    }
}

fn push_day_log(state: &mut BrainState, observation: Observation, cap: usize) {
    state.day_log.push_back(observation);
    while state.day_log.len() > cap {
        state.day_log.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    use super::memory::FakeEmbedder;
    use super::observe::Cast;
    use super::schedule::{hm, ENERGY_FULL, TICKS_PER_DAY, TICKS_PER_GAME_MINUTE as TPM};

    /// Answers from a script, counting the calls. The only "model" in these
    /// tests; the `Coordinator` path has its own test below.
    #[derive(Debug)]
    struct ScriptedThinker {
        answers: Mutex<VecDeque<String>>,
        calls: AtomicUsize,
        delay_ms: u64,
    }

    impl ScriptedThinker {
        fn new(answers: &[&str]) -> ScriptedThinker {
            ScriptedThinker {
                answers: Mutex::new(answers.iter().map(|s| s.to_string()).collect()),
                calls: AtomicUsize::new(0),
                delay_ms: 0,
            }
        }
        fn slow(mut self, ms: u64) -> ScriptedThinker {
            self.delay_ms = ms;
            self
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl Thinker for ScriptedThinker {
        async fn ask(&self, _prompt: &str) -> Result<String, BrainError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            let answer = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| "22:00 | bed | sleep".to_string());
            Ok(answer)
        }
    }

    fn places() -> Places {
        Places {
            bed: ObjectId(1),
            workbench: ObjectId(2),
            table: ObjectId(3),
            sofa: ObjectId(4),
        }
    }

    fn brain() -> Brain {
        Brain::new(
            "ada",
            Cast::new(EntId(1), "Ada").with_agent(EntId(2), "Grace"),
            places(),
            Arc::new(InMemoryStore::new()),
            Arc::new(FakeEmbedder),
        )
    }

    const PLAN: &str =
        "07:30 | table | breakfast\n08:00 | workbench | fixing the radio\n22:00 | bed | sleep";

    #[test]
    fn the_mirror_tags_match_the_world() {
        assert_eq!(Decision::GoTo(Tile::new(0, 0)).tag(), 0);
        assert_eq!(Decision::Sleep.tag(), 2);
        assert_eq!(Decision::Idle.tag(), 6);
        assert_eq!(
            WorldEvent::Arrived {
                ent: EntId(1),
                at: Tile::new(0, 0)
            }
            .tag(),
            Some(0)
        );
        assert_eq!(
            WorldEvent::Rejected {
                ent: EntId(1),
                code: "x".into()
            }
            .tag(),
            Some(12)
        );
        assert_eq!(WorldEvent::Dawn.tag(), None, "Dawn never goes on the wire");
    }

    #[test]
    fn ticking_costs_no_model_call() {
        let brain = brain();
        for tick in 0..2_000u64 {
            brain.tick_hint(tick);
        }
        assert_eq!(brain.model_calls(), 0);
        assert!(!brain.is_thinking());
    }

    #[test]
    fn the_agenda_answers_once_per_change() {
        let brain = brain();
        let mut answers = 0;
        // A whole game day, tick by game minute.
        for minute in 0..1440u64 {
            if brain.tick_hint(minute * TPM).is_some() {
                answers += 1;
            }
        }
        assert!(answers >= 5, "{answers}");
        assert!(
            answers <= 12,
            "one per schedule change, not per tick: {answers}"
        );
    }

    #[test]
    fn observations_are_queued_named_and_deduplicated() {
        let brain = brain();
        brain.set_tick(100);
        let kept = brain.observe(&[
            WorldEvent::Said {
                ent: EntId(2),
                text: "the kettle is on".into(),
            },
            WorldEvent::Said {
                ent: EntId(2),
                text: "the kettle is on".into(),
            },
            WorldEvent::StartedAnim {
                ent: EntId(2),
                anim: 1,
                dir: 0,
            },
        ]);
        assert_eq!(kept, 1);
        assert_eq!(brain.queue_len(), 1);
    }

    #[test]
    fn a_tool_call_sends_the_agent_to_the_workbench() {
        let brain = brain();
        let decisions = brain.observe_bus(&BusEvent::ToolCalled {
            agent: "ada".into(),
            tool: "yt-dlp".into(),
            ok: true,
            ms: 12,
        });
        assert_eq!(decisions, vec![Decision::Work(ObjectId(2))]);
        // The same event again does not refill the mailbox.
        let again = brain.observe_bus(&BusEvent::ToolCalled {
            agent: "ada".into(),
            tool: "yt-dlp".into(),
            ok: true,
            ms: 12,
        });
        assert!(again.is_empty());
        assert_eq!(brain.model_calls(), 0);
        // Another agent's work is not this agent's business.
        assert!(brain
            .observe_bus(&BusEvent::ToolCalled {
                agent: "grace".into(),
                tool: "ffmpeg".into(),
                ok: true,
                ms: 3,
            })
            .is_empty());
    }

    #[test]
    fn energy_makes_the_agent_yawn_once() {
        let brain = brain();
        assert!(brain.set_energy(200).is_empty());
        let said = brain.set_energy(60);
        assert_eq!(said.len(), 1);
        match &said[0] {
            Decision::Say(line) => assert!(line.len() <= prompt::MAX_SAY_BYTES),
            other => panic!("{other:?}"),
        }
        assert!(brain.set_energy(40).is_empty(), "only on the crossing");
    }

    #[test]
    fn capacity_is_energy() {
        let brain = brain();
        brain.set_capacity(&Capacity::with_quota(0.0), None);
        assert_eq!(brain.energy(), 0);
        brain.set_capacity(&Capacity::with_quota(1.0), None);
        assert_eq!(brain.energy(), ENERGY_FULL);
    }

    #[tokio::test]
    async fn zero_energy_never_calls_a_model() {
        let brain = brain();
        brain.set_tick(TICKS_PER_DAY + 8 * 60 * 9);
        brain.observe(&[WorldEvent::Said {
            ent: EntId(2),
            text: "wake up".into(),
        }]);
        brain.set_energy(0);
        let thinker = ScriptedThinker::new(&[PLAN]);
        assert!(!brain.should_think(brain.tick()));
        let err = brain.think(&thinker).await.unwrap_err();
        assert_eq!(err.code, ERR_BRAIN_NO_ENERGY);
        assert_eq!(thinker.calls(), 0);
        assert_eq!(brain.model_calls(), 0);
    }

    #[tokio::test]
    async fn an_idle_agent_does_not_think() {
        let brain = brain();
        brain.set_tick(TICKS_PER_DAY + 8 * 60 * 9);
        brain.observe(&[WorldEvent::Said {
            ent: EntId(2),
            text: "morning".into(),
        }]);
        assert!(brain.should_think(brain.tick()), "fresh observation");
        // Ten game minutes later with nothing new: idle.
        let later = brain.tick() + 10 * TPM;
        assert!(!brain.should_think(later));
    }

    #[tokio::test]
    async fn a_sleeping_agent_does_not_think() {
        let brain = brain();
        let night = TICKS_PER_DAY + u64::from(hm(23, 0)) * TPM;
        brain.set_tick(night);
        brain.observe(&[WorldEvent::Said {
            ent: EntId(2),
            text: "psst".into(),
        }]);
        assert!(!brain.should_think(night));
    }

    #[tokio::test]
    async fn think_plans_the_day_and_stores_the_observations() {
        let brain = brain();
        brain.set_tick(TICKS_PER_DAY + u64::from(hm(9, 0)) * TPM);
        brain.observe(&[
            WorldEvent::Said {
                ent: EntId(2),
                text: "the radio is broken".into(),
            },
            WorldEvent::Interacted {
                ent: EntId(2),
                object: ObjectId(2),
            },
        ]);
        let thinker = ScriptedThinker::new(&[PLAN]);
        assert!(brain.should_think(brain.tick()));
        let report = brain.think(&thinker).await.unwrap();
        assert_eq!(report.model_calls, 1);
        assert_eq!(report.memories_stored, 2);
        assert!(report.planned);
        assert!(!report.improvised);
        assert_eq!(brain.memory_count(), 2);
        assert_eq!(brain.queue_len(), 0);
        assert_eq!(brain.schedule().len(), 3);
        assert_eq!(
            brain.schedule().current(hm(9, 0)).unwrap().decision,
            Decision::Work(ObjectId(2))
        );
        assert!(report
            .decisions
            .iter()
            .any(|d| matches!(d, Decision::Say(s) if s.starts_with("Today:"))));
        assert!(report.decisions.contains(&Decision::Work(ObjectId(2))));
        // Same day again: no second plan, no second call.
        let second = brain.think(&thinker).await.unwrap();
        assert_eq!(second.model_calls, 0);
        assert_eq!(thinker.calls(), 1);
    }

    #[tokio::test]
    async fn an_unreadable_plan_falls_back_to_the_default_day() {
        let brain = brain();
        brain.set_tick(TICKS_PER_DAY + u64::from(hm(9, 0)) * TPM);
        brain.observe(&[WorldEvent::Said {
            ent: EntId(2),
            text: "hello".into(),
        }]);
        let thinker = ScriptedThinker::new(&["I would rather not say."]);
        let report = brain.think(&thinker).await.unwrap();
        assert!(report.improvised);
        assert_eq!(brain.schedule().len(), 8, "the default day");
    }

    #[tokio::test]
    async fn reflection_happens_once_a_day_and_consolidates() {
        let brain = brain();
        brain.set_tick(u64::from(hm(20, 0)) * TPM);
        for i in 0..8 {
            brain.observe(&[WorldEvent::Said {
                ent: EntId(2),
                text: format!("thing {i}"),
            }]);
        }
        // Day 1, morning: the reflection about day 0 comes first.
        brain.set_tick(TICKS_PER_DAY + u64::from(hm(9, 0)) * TPM);
        let thinker =
            ScriptedThinker::new(&["8 | the radio always breaks\n4 | Grace talks a lot", PLAN]);
        let reflected = brain.think(&thinker).await.unwrap();
        assert_eq!(reflected.model_calls, 1);
        assert_eq!(reflected.reflections, 2);
        assert_eq!(reflected.memories_stored, 8);
        assert!(!reflected.planned, "one turn per think");
        assert_eq!(brain.memory_count(), 10, "8 observations + 2 reflections");

        // The next think plans; reflection does not repeat on the same day.
        let planned = brain.think(&thinker).await.unwrap();
        assert_eq!(planned.model_calls, 1);
        assert_eq!(planned.reflections, 0);
        assert!(planned.planned);
        assert_eq!(thinker.calls(), 2);

        // And a third think on the same day costs nothing at all.
        let third = brain.think(&thinker).await.unwrap();
        assert_eq!(third.model_calls, 0);
        assert_eq!(thinker.calls(), 2);
    }

    #[tokio::test]
    async fn only_one_turn_is_in_flight_per_agent() {
        let brain = Arc::new(brain());
        brain.set_tick(TICKS_PER_DAY + u64::from(hm(9, 0)) * TPM);
        brain.observe(&[WorldEvent::Said {
            ent: EntId(2),
            text: "hello".into(),
        }]);
        let thinker = Arc::new(ScriptedThinker::new(&[PLAN, PLAN]).slow(60));
        let (a, b) = {
            let (b1, t1) = (brain.clone(), thinker.clone());
            let first = tokio::spawn(async move { b1.think(t1.as_ref()).await });
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let (b2, t2) = (brain.clone(), thinker.clone());
            let second = tokio::spawn(async move { b2.think(t2.as_ref()).await });
            (first.await.unwrap(), second.await.unwrap())
        };
        assert!(a.is_ok(), "{a:?}");
        assert_eq!(b.unwrap_err().code, ERR_BRAIN_BUSY);
        assert_eq!(
            thinker.calls(),
            1,
            "the second caller never reached a model"
        );
        assert!(!brain.is_thinking(), "the flag is released");
    }

    #[tokio::test]
    async fn the_flag_is_released_when_the_model_fails() {
        #[derive(Debug)]
        struct Failing;
        #[async_trait]
        impl Thinker for Failing {
            async fn ask(&self, _prompt: &str) -> Result<String, BrainError> {
                Err(LlmError::new(super::super::error::ERR_LLM_RATE, "slow down").into())
            }
        }
        let brain = brain();
        brain.set_tick(TICKS_PER_DAY + u64::from(hm(9, 0)) * TPM);
        brain.observe(&[WorldEvent::Said {
            ent: EntId(2),
            text: "hello".into(),
        }]);
        let err = brain.think(&Failing).await.unwrap_err();
        assert_eq!(err.code, ERR_BRAIN_MODEL);
        assert!(err.message.contains("ERR_LLM_RATE"));
        assert!(!brain.is_thinking());
        // The observations were still memorised: the day is not lost.
        assert_eq!(brain.memory_count(), 1);
    }

    #[tokio::test]
    async fn dawn_wakes_the_agent_and_is_remembered() {
        let brain = brain();
        brain.set_tick(TICKS_PER_DAY + u64::from(hm(9, 0)) * TPM);
        brain.set_energy(0);
        assert_eq!(brain.tick_hint(brain.tick()), Some(Decision::Sleep));
        brain.observe(&[WorldEvent::Dawn]);
        brain.set_energy(ENERGY_FULL);
        assert_eq!(
            brain.tick_hint(brain.tick()),
            Some(Decision::Work(ObjectId(2))),
            "back to the agenda"
        );
        assert_eq!(brain.queue_len(), 1);
    }

    #[tokio::test]
    async fn the_coordinator_thinker_collects_a_turn() {
        use crate::core::llm::agent::{AgentRole, Budget, ModelPolicy, RuntimeKind};
        use crate::core::llm::broker::{ToolBroker, ToolExecutor};
        use crate::core::llm::budget::BudgetStore;
        use crate::core::llm::providers::fake::FakeProvider;
        use crate::core::llm::runtime::NativeRuntime;
        use crate::core::llm::types::{ModelRef, ProviderId};
        use crate::core::omni::bus::Bus;

        #[derive(Debug)]
        struct NoTools;
        #[async_trait]
        impl ToolExecutor for NoTools {
            async fn execute(
                &self,
                _name: &str,
                _input: serde_json::Value,
            ) -> Result<String, LlmError> {
                Err(LlmError::stub())
            }
        }

        let native = NativeRuntime::new();
        native.register(
            "fake",
            Arc::new(FakeProvider::text("07:30 | table | breakfast", 3)),
        );
        let bus = Arc::new(Bus::new());
        let broker = Arc::new(ToolBroker::new(Vec::new(), Arc::new(NoTools), bus.clone()));
        let coordinator = Arc::new(
            Coordinator::new(
                Arc::new(native),
                broker,
                Arc::new(BudgetStore::memory()),
                bus,
            )
            .with_dir(None),
        );
        let agent = AgentDef {
            id: "ada".into(),
            name: "Ada".into(),
            role: AgentRole::Worker,
            system_prompt: "you live in a house".into(),
            model: ModelPolicy::Fixed {
                model: ModelRef {
                    provider: ProviderId::new("fake"),
                    model: "fake-1".into(),
                },
            },
            tools: Vec::new(),
            skills: Vec::new(),
            budget: Budget::default(),
            runtime: RuntimeKind::Native,
            skin: None,
        };
        let thinker = CoordinatorThinker::new(coordinator, agent);
        let answer = thinker.ask("plan your day").await.unwrap();
        assert_eq!(answer, "07:30 | table | breakfast");

        // And the brain can plan from it end to end.
        let brain = brain();
        brain.set_tick(TICKS_PER_DAY + u64::from(hm(9, 0)) * TPM);
        brain.observe(&[WorldEvent::Said {
            ent: EntId(2),
            text: "good morning".into(),
        }]);
        let report = brain.think(&thinker).await.unwrap();
        assert!(report.planned);
        assert!(!report.improvised);
    }

    /// Observe → memorise → reflect → plan, end to end, on the fake runtime
    /// (`providers::fake`). No network, no key, no real model: the point is
    /// that the cycle closes and that every step leaves something behind that
    /// the next one can read.
    #[tokio::test]
    async fn the_whole_cycle_runs_on_the_fake_runtime() {
        let day_one = TICKS_PER_DAY + u64::from(hm(9, 0)) * TPM;
        let reflect_thinker = fake_thinker(
            "reflect",
            "8 | the radio always breaks\n4 | Grace works at the bench",
        );
        let plan_thinker = fake_thinker("plan", PLAN);

        let brain = brain();
        // Day 0: a day's worth of things happen.
        brain.set_tick(u64::from(hm(20, 0)) * TPM);
        for i in 0..6 {
            brain.observe(&[WorldEvent::Said {
                ent: EntId(2),
                text: format!("the radio is broken again ({i})"),
            }]);
        }
        brain.observe_bus(&BusEvent::ToolCalled {
            agent: "ada".into(),
            tool: "yt-dlp".into(),
            ok: true,
            ms: 8,
        });
        assert_eq!(brain.queue_len(), 7);
        assert_eq!(brain.model_calls(), 0, "a day of living costs nothing");

        // Day 1, 09:00: one turn, and it is the reflection.
        brain.set_tick(day_one);
        assert!(brain.should_think(day_one));
        let reflected = brain.think(&reflect_thinker).await.unwrap();
        assert_eq!(reflected.model_calls, 1);
        assert_eq!(reflected.memories_stored, 7);
        assert_eq!(reflected.reflections, 2);
        assert!(!reflected.planned, "one turn per think");
        assert_eq!(brain.memory_count(), 9);

        // The next turn is the plan, and it is written from the memories.
        let planned = brain.think(&plan_thinker).await.unwrap();
        assert_eq!(planned.model_calls, 1);
        assert!(planned.planned && !planned.improvised);
        assert_eq!(
            brain.schedule().current(hm(9, 0)).unwrap().decision,
            Decision::Work(ObjectId(2)),
            "09:00 is workbench time, as the model asked"
        );

        // And what was learned is retrievable, reflections included.
        let query = FakeEmbedder::vector("radio");
        let got = memory::retrieve(brain.store.as_ref(), "ada", &query, brain.tick(), 4).unwrap();
        assert!(got.iter().any(|m| m.text.contains("radio")));
        assert!(
            got.iter().any(|m| m.kind == MemoryKind::Reflection),
            "the day was consolidated into something worth remembering: {got:?}"
        );
        // Two turns for a whole day of world: that is the budget.
        assert_eq!(brain.model_calls(), 2);
    }

    /// A [`Thinker`] over the real `Coordinator` with the fake provider behind
    /// it, so the test exercises the turn plumbing and not just the parser.
    fn fake_thinker(name: &str, answer: &str) -> CoordinatorThinker {
        use crate::core::llm::agent::{AgentRole, Budget, ModelPolicy, RuntimeKind};
        use crate::core::llm::broker::{ToolBroker, ToolExecutor};
        use crate::core::llm::budget::BudgetStore;
        use crate::core::llm::providers::fake::FakeProvider;
        use crate::core::llm::runtime::NativeRuntime;
        use crate::core::llm::types::{ModelRef, ProviderId};
        use crate::core::omni::bus::Bus;

        #[derive(Debug)]
        struct NoTools;
        #[async_trait]
        impl ToolExecutor for NoTools {
            async fn execute(
                &self,
                _name: &str,
                _input: serde_json::Value,
            ) -> Result<String, LlmError> {
                Err(LlmError::stub())
            }
        }

        let native = NativeRuntime::new();
        native.register(name, Arc::new(FakeProvider::text(answer, 2)));
        let bus = Arc::new(Bus::new());
        let broker = Arc::new(ToolBroker::new(Vec::new(), Arc::new(NoTools), bus.clone()));
        let coordinator = Arc::new(
            Coordinator::new(
                Arc::new(native),
                broker,
                Arc::new(BudgetStore::memory()),
                bus,
            )
            .with_dir(None),
        );
        let agent = AgentDef {
            id: "ada".into(),
            name: "Ada".into(),
            role: AgentRole::Worker,
            system_prompt: "you live in a house".into(),
            model: ModelPolicy::Fixed {
                model: ModelRef {
                    provider: ProviderId::new(name),
                    model: "fake-1".into(),
                },
            },
            tools: Vec::new(),
            skills: Vec::new(),
            budget: Budget::default(),
            runtime: RuntimeKind::Native,
            skin: None,
        };
        CoordinatorThinker::new(coordinator, agent).with_conversation(format!("brain-{name}"))
    }

    #[tokio::test]
    async fn memories_are_retrievable_after_a_think() {
        let brain = brain();
        brain.set_tick(TICKS_PER_DAY + u64::from(hm(9, 0)) * TPM);
        brain.observe(&[
            WorldEvent::Said {
                ent: EntId(2),
                text: "the radio is broken".into(),
            },
            WorldEvent::Said {
                ent: EntId(2),
                text: "the cat is on the sofa".into(),
            },
        ]);
        brain.think(&ScriptedThinker::new(&[PLAN])).await.unwrap();
        let query = FakeEmbedder::vector("radio");
        let got = memory::retrieve(brain.store.as_ref(), "ada", &query, brain.tick(), 1).unwrap();
        assert!(got[0].text.contains("radio"), "{:?}", got[0].text);
        assert!(got[0].embedding.is_some(), "embedded in the batch");
    }

    #[test]
    fn a_detached_brain_works_with_nothing_wired() {
        let brain = Brain::detached("omni", EntId(9), "Omni");
        assert_eq!(brain.ent(), EntId(9));
        assert_eq!(brain.energy(), ENERGY_FULL);
        assert!(brain.tick_hint(0).is_some());
        assert_eq!(brain.model_calls(), 0);
        assert!(format!("{brain:?}").contains("omni"));
    }
}
