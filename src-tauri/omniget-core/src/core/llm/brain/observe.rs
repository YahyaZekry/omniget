//! Observation: world events and bus events become sentences an agent can
//! remember. Owned by f7-world-brain.
//!
//! Two sources feed the same queue:
//!
//! * `WorldEvent` from the tick — who arrived, who said what, who went to bed.
//! * `BusEvent` from the LLM stack (f2 `core/omni/bus.rs`) — the diegetic
//!   half of plan §9.1: a tool call by this agent is the agent walking to the
//!   workbench, a queued download is a box carried to the shelf.
//!
//! Nothing here calls a model and nothing here allocates per tick unless an
//! event actually arrives. The queue is bounded: a house that runs all night
//! with the route closed cannot grow memory without bound.

use std::collections::{BTreeMap, VecDeque};

use super::schedule::Places;
use super::{Decision, EntId, ObjectId, WorldEvent};
use crate::core::omni::bus::BusEvent;

/// How many observations wait for the next `think`. Past this the oldest
/// falls off, exactly like the world's mailbox.
pub const QUEUE_CAP: usize = 64;

/// Anything at or above this is worth waking an idle agent for.
pub const SALIENT: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    World,
    Bus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub text: String,
    /// 0..=10, the same scale as `memory::Memory::importance`.
    pub importance: u8,
    pub tick: u64,
    pub source: Source,
}

impl Observation {
    pub fn new(text: impl Into<String>, importance: u8, tick: u64, source: Source) -> Observation {
        Observation {
            text: text.into(),
            importance: importance.min(super::memory::MAX_IMPORTANCE),
            tick,
            source,
        }
    }

    pub fn is_salient(&self) -> bool {
        self.importance >= SALIENT
    }
}

/// Who is who, so an event becomes a sentence instead of a row of ids. The
/// bridge fills it from the roster and from `house-v1.json`; the brain never
/// invents a name.
#[derive(Debug, Clone, Default)]
pub struct Cast {
    pub me: EntId,
    pub my_name: String,
    pub agents: BTreeMap<u32, String>,
    pub objects: BTreeMap<u32, String>,
}

impl Cast {
    pub fn new(me: EntId, my_name: impl Into<String>) -> Cast {
        Cast {
            me,
            my_name: my_name.into(),
            agents: BTreeMap::new(),
            objects: BTreeMap::new(),
        }
    }

    pub fn with_agent(mut self, ent: EntId, name: impl Into<String>) -> Cast {
        self.agents.insert(ent.0, name.into());
        self
    }

    pub fn with_object(mut self, object: ObjectId, name: impl Into<String>) -> Cast {
        self.objects.insert(object.0, name.into());
        self
    }

    /// `"I"` for this agent, the roster name for anyone known, `"someone"`
    /// otherwise — never a raw id in a sentence a model reads.
    pub fn who(&self, ent: EntId) -> String {
        if ent == self.me {
            return "I".to_string();
        }
        self.agents
            .get(&ent.0)
            .cloned()
            .unwrap_or_else(|| "someone".to_string())
    }

    pub fn what(&self, object: ObjectId) -> String {
        self.objects
            .get(&object.0)
            .cloned()
            .unwrap_or_else(|| "something".to_string())
    }

    fn is_me(&self, ent: EntId) -> bool {
        ent == self.me
    }
}

/// Turn a world event into an observation. `None` means "not worth a memory":
/// animation starts, catch-up markers and an agent's own arrival at a tile it
/// asked to walk to are noise, and noise costs tokens later.
pub fn observe_event(cast: &Cast, event: &WorldEvent, tick: u64) -> Option<Observation> {
    let (text, importance) = match event {
        WorldEvent::StartedAnim { .. } | WorldEvent::CaughtUp { .. } => return None,
        WorldEvent::Arrived { ent, at } => {
            if cast.is_me(*ent) {
                return None;
            }
            (
                format!("{} arrived at ({}, {})", cast.who(*ent), at.x, at.y),
                1,
            )
        }
        WorldEvent::Said { ent, text } => {
            if cast.is_me(*ent) {
                (format!("I said \"{text}\""), 2)
            } else {
                (format!("{} said \"{}\"", cast.who(*ent), text), 4)
            }
        }
        WorldEvent::Interacted { ent, object } => (
            format!("{} used the {}", cast.who(*ent), cast.what(*object)),
            3,
        ),
        WorldEvent::Slept { ent } => (format!("{} went to sleep", cast.who(*ent)), 2),
        WorldEvent::Woke { ent } => (format!("{} woke up", cast.who(*ent)), 2),
        WorldEvent::Yawned { ent } => (format!("{} yawned", cast.who(*ent)), 2),
        WorldEvent::Spawned { ent, .. } => {
            if cast.is_me(*ent) {
                return None;
            }
            (format!("{} came into the house", cast.who(*ent)), 5)
        }
        WorldEvent::Despawned { ent } => {
            if cast.is_me(*ent) {
                return None;
            }
            (format!("{} left the house", cast.who(*ent)), 4)
        }
        WorldEvent::ObjectPlaced { object, at } => (
            format!(
                "a {} was placed at ({}, {})",
                cast.what(*object),
                at.x,
                at.y
            ),
            3,
        ),
        WorldEvent::ObjectRemoved { object } => {
            (format!("the {} was taken away", cast.what(*object)), 3)
        }
        WorldEvent::Rejected { ent, code } => {
            if !cast.is_me(*ent) {
                return None;
            }
            (format!("what I tried to do did not work ({code})"), 6)
        }
        WorldEvent::Dawn => ("the day started over and I have energy again".into(), 8),
    };
    Some(Observation::new(text, importance, tick, Source::World))
}

/// Turn a bus event into an observation. Events about other agents are
/// skipped: an agent remembers its own work and what happens to the house,
/// not the whole telemetry stream.
pub fn observe_bus(agent_id: &str, event: &BusEvent, tick: u64) -> Option<Observation> {
    let mine = |agent: &str| agent == agent_id;
    let (text, importance) = match event {
        BusEvent::TurnStarted { agent, .. } if mine(agent) => {
            ("I started working on a request".to_string(), 3)
        }
        BusEvent::TurnEnded { agent, usage } if mine(agent) => (
            format!(
                "I finished a request ({} tokens in, {} out)",
                usage.input_tokens, usage.output_tokens
            ),
            3,
        ),
        BusEvent::ToolCalled {
            agent,
            tool,
            ok,
            ms,
            ..
        } if mine(agent) => {
            if *ok {
                (
                    format!("I ran the tool {tool} at the workbench ({ms} ms)"),
                    5,
                )
            } else {
                (format!("the tool {tool} failed on me"), 7)
            }
        }
        BusEvent::ToolAsk { agent, tool, .. } if mine(agent) => {
            (format!("I asked for permission to use {tool}"), 6)
        }
        BusEvent::BudgetHit { agent } if mine(agent) => {
            ("I ran out of budget for today".to_string(), 9)
        }
        BusEvent::Rerouted {
            agent, from, to, ..
        } if mine(agent) => (format!("I switched from {from} to {to}"), 6),
        BusEvent::DownloadQueued { id } => (format!("a download (#{id}) joined the queue"), 2),
        BusEvent::DownloadFinished { id } => (format!("the download #{id} finished"), 2),
        BusEvent::DownloadFailed { id, code } => {
            (format!("the download #{id} failed with {code}"), 5)
        }
        _ => return None,
    };
    Some(Observation::new(text, importance, tick, Source::Bus))
}

/// The diegetic half of plan §9.1: what the agent does in the house because
/// of what it is doing in the app. Costs zero tokens — it is a pure map from
/// a bus event to a world decision.
pub fn work_decision(agent_id: &str, event: &BusEvent, places: &Places) -> Option<Decision> {
    match event {
        BusEvent::TurnStarted { agent, .. } | BusEvent::ToolCalled { agent, .. }
            if agent == agent_id =>
        {
            Some(Decision::Work(places.workbench))
        }
        BusEvent::TurnEnded { agent, .. } if agent == agent_id => Some(Decision::Sit(places.sofa)),
        BusEvent::BudgetHit { agent } if agent == agent_id => Some(Decision::Sleep),
        _ => None,
    }
}

/// The bounded queue of observations waiting for the next `think`.
#[derive(Debug, Clone, Default)]
pub struct ObserveQueue {
    items: VecDeque<Observation>,
    dropped: u64,
    last_tick: u64,
}

impl ObserveQueue {
    pub fn new() -> ObserveQueue {
        ObserveQueue::default()
    }

    /// Push an observation, dropping the oldest when full and swallowing an
    /// exact repeat of the newest one. Returns `false` when it was swallowed.
    pub fn push(&mut self, observation: Observation) -> bool {
        if self
            .items
            .back()
            .is_some_and(|last| last.text == observation.text)
        {
            return false;
        }
        self.last_tick = self.last_tick.max(observation.tick);
        self.items.push_back(observation);
        while self.items.len() > QUEUE_CAP {
            self.items.pop_front();
            self.dropped += 1;
        }
        true
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// How many observations fell off the back for want of room.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The most recent tick this queue saw anything at.
    pub fn last_tick(&self) -> u64 {
        self.last_tick
    }

    pub fn iter(&self) -> impl Iterator<Item = &Observation> {
        self.items.iter()
    }

    /// Anything worth waking up for?
    pub fn has_salient(&self) -> bool {
        self.items.iter().any(Observation::is_salient)
    }

    /// Take everything, leaving the queue empty and the counters intact.
    pub fn drain(&mut self) -> Vec<Observation> {
        self.items.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::Tile;
    use super::*;
    use crate::core::llm::types::Usage;

    fn cast() -> Cast {
        Cast::new(EntId(1), "Ada")
            .with_agent(EntId(2), "Grace")
            .with_object(ObjectId(7), "workbench")
    }

    #[test]
    fn world_events_become_sentences_with_names() {
        let c = cast();
        let said = observe_event(
            &c,
            &WorldEvent::Said {
                ent: EntId(2),
                text: "the kettle is on".into(),
            },
            10,
        )
        .unwrap();
        assert_eq!(said.text, "Grace said \"the kettle is on\"");
        assert_eq!(said.importance, 4);
        assert_eq!(said.tick, 10);

        let mine = observe_event(
            &c,
            &WorldEvent::Said {
                ent: EntId(1),
                text: "on my way".into(),
            },
            10,
        )
        .unwrap();
        assert_eq!(mine.text, "I said \"on my way\"");
        assert!(
            mine.importance < said.importance,
            "my own words matter less"
        );

        let used = observe_event(
            &c,
            &WorldEvent::Interacted {
                ent: EntId(2),
                object: ObjectId(7),
            },
            11,
        )
        .unwrap();
        assert_eq!(used.text, "Grace used the workbench");
    }

    #[test]
    fn unknown_ids_never_leak_into_a_prompt() {
        let c = cast();
        let obs = observe_event(
            &c,
            &WorldEvent::Interacted {
                ent: EntId(9),
                object: ObjectId(9),
            },
            0,
        )
        .unwrap();
        assert_eq!(obs.text, "someone used the something");
        assert!(!obs.text.contains('9'));
    }

    #[test]
    fn noise_is_dropped() {
        let c = cast();
        assert!(observe_event(
            &c,
            &WorldEvent::StartedAnim {
                ent: EntId(2),
                anim: 1,
                dir: 0
            },
            0
        )
        .is_none());
        assert!(observe_event(&c, &WorldEvent::CaughtUp { ticks: 500 }, 0).is_none());
        // My own arrival is not news: I asked to walk there.
        assert!(observe_event(
            &c,
            &WorldEvent::Arrived {
                ent: EntId(1),
                at: Tile::new(3, 4)
            },
            0
        )
        .is_none());
        assert!(observe_event(
            &c,
            &WorldEvent::Arrived {
                ent: EntId(2),
                at: Tile::new(3, 4)
            },
            0
        )
        .is_some());
        // Somebody else's decision being refused is not my business.
        assert!(observe_event(
            &c,
            &WorldEvent::Rejected {
                ent: EntId(2),
                code: "ERR_WORLD_NO_PATH".into()
            },
            0
        )
        .is_none());
    }

    #[test]
    fn dawn_is_the_most_important_thing_that_happens() {
        let obs = observe_event(&cast(), &WorldEvent::Dawn, 100).unwrap();
        assert_eq!(obs.importance, 8);
        assert!(obs.is_salient());
    }

    #[test]
    fn bus_events_of_other_agents_are_ignored() {
        let mine = observe_bus(
            "ada",
            &BusEvent::ToolCalled {
                agent: "ada".into(),
                tool: "yt-dlp".into(),
                ok: true,
                ms: 120,
            },
            5,
        )
        .unwrap();
        assert!(mine.text.contains("yt-dlp"));
        assert_eq!(mine.source, Source::Bus);
        assert!(observe_bus(
            "ada",
            &BusEvent::ToolCalled {
                agent: "grace".into(),
                tool: "yt-dlp".into(),
                ok: true,
                ms: 120,
            },
            5,
        )
        .is_none());
        assert!(observe_bus("ada", &BusEvent::Idle { seconds: 30 }, 5).is_none());
        assert!(observe_bus(
            "ada",
            &BusEvent::TokenDelta {
                agent: "ada".into(),
                chars: 4
            },
            5
        )
        .is_none());
    }

    #[test]
    fn a_failed_tool_is_more_memorable_than_a_good_one() {
        let ok = observe_bus(
            "ada",
            &BusEvent::ToolCalled {
                agent: "ada".into(),
                tool: "ffmpeg".into(),
                ok: true,
                ms: 10,
            },
            0,
        )
        .unwrap();
        let bad = observe_bus(
            "ada",
            &BusEvent::ToolCalled {
                agent: "ada".into(),
                tool: "ffmpeg".into(),
                ok: false,
                ms: 10,
            },
            0,
        )
        .unwrap();
        assert!(bad.importance > ok.importance);
        let broke = observe_bus(
            "ada",
            &BusEvent::BudgetHit {
                agent: "ada".into(),
            },
            0,
        )
        .unwrap();
        assert_eq!(broke.importance, 9);
    }

    #[test]
    fn the_house_notices_downloads_whoever_asked() {
        let obs = observe_bus("ada", &BusEvent::DownloadQueued { id: 42 }, 0).unwrap();
        assert!(obs.text.contains("#42"));
        let failed = observe_bus(
            "ada",
            &BusEvent::DownloadFailed {
                id: 7,
                code: "ERR_NET".into(),
            },
            0,
        )
        .unwrap();
        assert!(failed.is_salient());
    }

    #[test]
    fn work_is_diegetic() {
        let places = Places {
            bed: ObjectId(1),
            workbench: ObjectId(7),
            table: ObjectId(3),
            sofa: ObjectId(4),
        };
        assert_eq!(
            work_decision(
                "ada",
                &BusEvent::TurnStarted {
                    agent: "ada".into(),
                    conversation: "c".into()
                },
                &places
            ),
            Some(Decision::Work(ObjectId(7)))
        );
        assert_eq!(
            work_decision(
                "ada",
                &BusEvent::TurnEnded {
                    agent: "ada".into(),
                    usage: Usage::default()
                },
                &places
            ),
            Some(Decision::Sit(ObjectId(4)))
        );
        assert_eq!(
            work_decision(
                "ada",
                &BusEvent::BudgetHit {
                    agent: "ada".into()
                },
                &places
            ),
            Some(Decision::Sleep)
        );
        assert_eq!(
            work_decision(
                "ada",
                &BusEvent::TurnStarted {
                    agent: "grace".into(),
                    conversation: "c".into()
                },
                &places
            ),
            None
        );
    }

    #[test]
    fn the_queue_is_bounded_and_swallows_repeats() {
        let mut q = ObserveQueue::new();
        assert!(q.is_empty());
        assert!(q.push(Observation::new("the kettle boiled", 3, 1, Source::World)));
        assert!(
            !q.push(Observation::new("the kettle boiled", 3, 2, Source::World)),
            "an exact repeat is not a second memory"
        );
        assert_eq!(q.len(), 1);
        for i in 0..QUEUE_CAP + 10 {
            q.push(Observation::new(format!("event {i}"), 1, 3, Source::World));
        }
        assert_eq!(q.len(), QUEUE_CAP);
        assert_eq!(q.dropped(), 11);
        assert_eq!(q.last_tick(), 3);
        assert!(!q.has_salient());
        q.push(Observation::new(
            "the house is on fire",
            9,
            4,
            Source::World,
        ));
        assert!(q.has_salient());
        let drained = q.drain();
        assert_eq!(drained.len(), QUEUE_CAP);
        assert!(q.is_empty());
        assert_eq!(q.dropped(), 12, "counters survive a drain");
    }
}
