//! The bridge between `core::llm::brain` and `omniget-world`. Owned by
//! f7-world-bridge / m4-world.
//!
//! `omniget-core` does not depend on `omniget-world`, so the brain carries
//! mirror types (`brain::Decision`, `brain::WorldEvent`, `brain::Tile`, …) with
//! the same variants in the same order as the crate's. The whole conversion is
//! therefore a `match` with no decisions in it, and it lives here — the one
//! place that links both crates — exactly as the brain's module doc asks.
//!
//! What this module adds on top of the conversion is [`Brains`]: one [`Brain`]
//! per agent in the house, fed by the tick thread. The budget rules are the
//! brain's own and are not re-implemented here:
//!
//! * **no model call inside the tick.** [`Brains::tick_hints`] only ever calls
//!   `Brain::tick_hint`, which is a pure function of (minute of the game day,
//!   energy, last decision). No `Thinker` is attached in this phase, so the
//!   number of model calls a running house makes is exactly zero and there is
//!   no network in the tick path at all.
//! * **the world is the authority on what happens.** The brain observes; it
//!   never writes the world behind the tick's back. Decisions leave as
//!   `Input::Decision` and go through the mailbox like everything else.
//! * **the bus drives the body through [`super::input::bus_input`]**, not
//!   through the brain: `Brain::observe_bus` also returns a work decision, and
//!   pushing both would put the same `Decision::Work` in the mailbox twice.
//!   Here the decision is dropped and only the memory is kept — but it is still
//!   called, because it is what teaches the brain that the agent is already at
//!   the bench, which is what stops `tick_hint` from sending it there again.

use std::collections::BTreeMap;
use std::sync::Arc;

use omniget_core::core::llm::brain::memory::{Embedder, MemoryStore};
use omniget_core::core::llm::brain::observe::Cast;
use omniget_core::core::llm::brain::schedule::Places;
use omniget_core::core::llm::brain::{
    Brain, Decision as BrainDecision, EntId as BrainEnt, ObjectId as BrainObject,
    Tile as BrainTile, WorldEvent as BrainEvent,
};
use omniget_core::core::omni::bus::BusEvent;
use omniget_world::{Decision, EntId, Input, ObjectId, Tile, WorldEvent};

/// Object kinds the default day needs, in the spelling `house-v1.json` uses.
const KIND_BED: &str = "object/bed";
const KIND_WORKBENCH: &str = "object/workbench";
const KIND_TABLE: &str = "object/table";
/// There is no sofa in `casa-v1`: the evening chair is where an agent sits.
const KIND_SOFA: &str = "object/chair";

fn to_world_tile(t: BrainTile) -> Tile {
    Tile::new(t.x, t.y)
}

fn to_brain_tile(t: Tile) -> BrainTile {
    BrainTile::new(t.x, t.y)
}

/// A brain decision as the world's mailbox takes it.
pub fn to_world_decision(d: &BrainDecision) -> Decision {
    match d {
        BrainDecision::GoTo(t) => Decision::GoTo(to_world_tile(*t)),
        BrainDecision::Sit(o) => Decision::Sit(ObjectId(o.0)),
        BrainDecision::Sleep => Decision::Sleep,
        BrainDecision::Work(o) => Decision::Work(ObjectId(o.0)),
        // The world truncates on a char boundary at 200 bytes; the brain has
        // already clamped it, so this is only ever a copy.
        BrainDecision::Say(text) => Decision::Say(text.clone()),
        BrainDecision::Wave(e) => Decision::Wave(EntId(e.0)),
        BrainDecision::Idle => Decision::Idle,
    }
}

/// A world event in the shape the brain observes. Total: a new variant in the
/// crate stops compiling here, which is the point of writing it as a `match`.
pub fn to_brain_event(e: &WorldEvent) -> BrainEvent {
    match e {
        WorldEvent::Arrived { ent, at } => BrainEvent::Arrived {
            ent: BrainEnt(ent.0),
            at: to_brain_tile(*at),
        },
        WorldEvent::StartedAnim { ent, anim, dir } => BrainEvent::StartedAnim {
            ent: BrainEnt(ent.0),
            anim: *anim,
            dir: *dir,
        },
        WorldEvent::Said { ent, text } => BrainEvent::Said {
            ent: BrainEnt(ent.0),
            text: text.clone(),
        },
        WorldEvent::Interacted { ent, object } => BrainEvent::Interacted {
            ent: BrainEnt(ent.0),
            object: BrainObject(object.0),
        },
        WorldEvent::Slept { ent } => BrainEvent::Slept {
            ent: BrainEnt(ent.0),
        },
        WorldEvent::Woke { ent } => BrainEvent::Woke {
            ent: BrainEnt(ent.0),
        },
        WorldEvent::Yawned { ent } => BrainEvent::Yawned {
            ent: BrainEnt(ent.0),
        },
        WorldEvent::Spawned { ent, at } => BrainEvent::Spawned {
            ent: BrainEnt(ent.0),
            at: to_brain_tile(*at),
        },
        WorldEvent::Despawned { ent } => BrainEvent::Despawned {
            ent: BrainEnt(ent.0),
        },
        WorldEvent::ObjectPlaced { object, at } => BrainEvent::ObjectPlaced {
            object: BrainObject(object.0),
            at: to_brain_tile(*at),
        },
        WorldEvent::ObjectRemoved { object } => BrainEvent::ObjectRemoved {
            object: BrainObject(object.0),
        },
        WorldEvent::CaughtUp { ticks } => BrainEvent::CaughtUp { ticks: *ticks },
        WorldEvent::Rejected { ent, code } => BrainEvent::Rejected {
            ent: BrainEnt(ent.0),
            code: code.to_string(),
        },
    }
}

/// The four objects a default day names, picked out of the map's objects.
/// Lowest id wins so two runs of the same house pick the same furniture; a
/// missing kind falls back to the workbench rather than to object 0, which
/// would be a decision about an object that does not exist.
pub fn places_from_objects(objects: &[(ObjectId, String)]) -> Option<Places> {
    let first = |kind: &str| -> Option<u32> {
        objects
            .iter()
            .filter(|(_, k)| k == kind)
            .map(|(id, _)| id.0)
            .min()
    };
    let workbench = first(KIND_WORKBENCH).or_else(|| first(KIND_TABLE))?;
    Some(Places {
        bed: BrainObject(first(KIND_BED).unwrap_or(workbench)),
        workbench: BrainObject(workbench),
        table: BrainObject(first(KIND_TABLE).unwrap_or(workbench)),
        sofa: BrainObject(first(KIND_SOFA).unwrap_or(workbench)),
    })
}

/// The embedder the house uses today.
///
/// Deliberately the deterministic local one: a real MiniLM session
/// (`core::embed`, f7-embed) downloads 23 MB the first time it runs, and
/// nothing the world does may reach the network without the user asking for it.
/// It costs nothing either way in this phase — an embedding is only ever
/// computed inside `Brain::think`, and no `Thinker` is attached yet, so the
/// call never happens. Swapping it is one line once the model is on disk.
pub fn default_embedder() -> Arc<dyn Embedder> {
    // The real MiniLM when its files are already on disk (the user installed
    // them once); otherwise the deterministic local one. Nothing here ever
    // downloads: reaching the network stays a user action.
    let on_disk = omniget_core::core::embed::minilm::model_path()
        .map(|p| p.exists())
        .unwrap_or(false);
    if on_disk {
        Arc::new(MiniLmEmbedder)
    } else {
        Arc::new(omniget_core::core::llm::brain::memory::FakeEmbedder)
    }
}

/// `core::embed` behind the brain's `Embedder`. The ONNX session is blocking,
/// so the batch runs on the blocking pool.
#[derive(Debug)]
pub struct MiniLmEmbedder;

#[async_trait::async_trait]
impl Embedder for MiniLmEmbedder {
    async fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Vec<f32>>, omniget_core::core::llm::brain::BrainError> {
        let owned: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
        let out = tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
            omniget_core::core::embed::embed_batch(&refs)
        })
        .await
        .map_err(|e| omniget_core::core::llm::brain::BrainError::embed(e.to_string()))?
        .map_err(|e| omniget_core::core::llm::brain::BrainError::embed(format!("{e:?}")))?;
        Ok(out.into_iter().map(|v| v.to_vec()).collect())
    }
}

/// A human-ish name for an object kind, for the sentences the brain writes
/// ("Grace used the workbench"). `object/workbench` → `workbench`.
fn object_label(kind: &str) -> &str {
    kind.rsplit('/').next().unwrap_or(kind)
}

/// One brain per agent in the house.
///
/// Empty until a world is installed, and empty again the moment it is deleted:
/// an app whose user never made a house holds no brain, no memory store and no
/// schedule. That is the same "Never = nothing allocated" rule the tick thread
/// follows, one level up.
#[derive(Default)]
pub struct Brains {
    agents: BTreeMap<u32, Arc<Brain>>,
    /// Last energy pushed in, so the 0 → awake crossing can be turned into
    /// `WorldEvent::Dawn` (plan §9.1) without the world knowing about quotas.
    energy: BTreeMap<u32, u8>,
}

impl std::fmt::Debug for Brains {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Brains")
            .field("agents", &self.agents.len())
            .finish()
    }
}

impl Brains {
    pub fn new() -> Brains {
        Brains::default()
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    pub fn get(&self, ent: EntId) -> Option<Arc<Brain>> {
        self.agents.get(&ent.0).cloned()
    }

    /// Forget everything. Called when the world is deleted.
    /// Every brain with its entity, for the thinker pump.
    pub fn iter(&self) -> impl Iterator<Item = (EntId, Arc<Brain>)> + '_ {
        self.agents.iter().map(|(id, b)| (EntId(*id), b.clone()))
    }

    pub fn clear(&mut self) {
        self.agents.clear();
        self.energy.clear();
    }

    /// Build the brains a house needs: one per agent, each knowing the others'
    /// names and the house's furniture. Agents that already have a brain keep
    /// it (with their memories and their plan for the day); agents that left
    /// lose theirs.
    ///
    /// `store` and `embedder` are shared by every brain — one SQLite file for
    /// the house, one embedding session — and the rows are keyed by agent id.
    pub fn rebuild(
        &mut self,
        agents: &[(EntId, String)],
        objects: &[(ObjectId, String)],
        store: &Arc<dyn MemoryStore>,
        embedder: &Arc<dyn Embedder>,
    ) {
        let Some(places) = places_from_objects(objects) else {
            // A house with no workbench and no table is not a house an agenda
            // can be written for. Better no brain than a plan about object 0.
            self.clear();
            return;
        };
        self.agents
            .retain(|id, _| agents.iter().any(|(e, _)| e.0 == *id));
        self.energy.retain(|id, _| self.agents.contains_key(id));
        for (ent, name) in agents {
            if self.agents.contains_key(&ent.0) {
                continue;
            }
            let mut cast = Cast::new(BrainEnt(ent.0), name.clone());
            for (other, other_name) in agents {
                if other != ent {
                    cast = cast.with_agent(BrainEnt(other.0), other_name.clone());
                }
            }
            for (id, kind) in objects {
                cast = cast.with_object(BrainObject(id.0), object_label(kind));
            }
            let brain = Brain::new(name.clone(), cast, places, store.clone(), embedder.clone());
            self.agents.insert(ent.0, Arc::new(brain));
        }
    }

    /// The tick told the brains what time it is.
    pub fn set_tick(&self, tick: u64) {
        for brain in self.agents.values() {
            brain.set_tick(tick);
        }
    }

    /// Everything that happened this tick, to everybody. `observe_event`
    /// already drops what is not worth a memory and knows which entity is
    /// "me", so no filtering happens here.
    pub fn observe(&self, tick: u64, events: &[WorldEvent]) -> usize {
        if self.agents.is_empty() || events.is_empty() {
            return 0;
        }
        let mirrored: Vec<BrainEvent> = events.iter().map(to_brain_event).collect();
        let mut kept = 0;
        for brain in self.agents.values() {
            brain.set_tick(tick);
            kept += brain.observe(&mirrored);
        }
        kept
    }

    /// A bus event, remembered by whoever it is about. The decision it also
    /// produces is deliberately dropped: [`super::input::bus_input`] is what
    /// moves the body (see the module doc).
    pub fn observe_bus(&self, tick: u64, event: &BusEvent) {
        for brain in self.agents.values() {
            brain.set_tick(tick);
            let _diegetic_work = brain.observe_bus(event);
        }
    }

    /// The agenda's answer for this tick, for every agent that has one. Pure:
    /// no model, no allocation unless a decision actually changed.
    pub fn tick_hints(&self, tick: u64) -> Vec<Input> {
        let mut out = Vec::new();
        for (id, brain) in &self.agents {
            if let Some(decision) = brain.tick_hint(tick) {
                out.push(Input::Decision {
                    ent: EntId(*id),
                    decision: to_world_decision(&decision),
                });
            }
        }
        out
    }

    /// Quota became energy. Returns whatever the agent says about it (a yawn
    /// on the way down), and synthesises `WorldEvent::Dawn` on the way back up
    /// from nothing, which is the event the brain waits for to plan again.
    pub fn set_energy(&mut self, ent: EntId, energy: u8) -> Vec<Input> {
        let Some(brain) = self.agents.get(&ent.0) else {
            return Vec::new();
        };
        let before = self.energy.insert(ent.0, energy);
        if before == Some(energy) {
            return Vec::new();
        }
        if before == Some(0) && energy > 0 {
            brain.observe(&[BrainEvent::Dawn]);
        }
        brain
            .set_energy(energy)
            .iter()
            .map(|d| Input::Decision {
                ent,
                decision: to_world_decision(d),
            })
            .collect()
    }

    /// Model calls every brain in the house has made. The tick budget is "this
    /// stays at zero while nobody attaches a `Thinker`".
    pub fn model_calls(&self) -> u32 {
        self.agents.values().map(|b| b.model_calls()).sum()
    }

    /// Memories held for every agent, for the HUD and the tests.
    pub fn memory_count(&self) -> usize {
        self.agents.values().map(|b| b.memory_count()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniget_core::core::llm::brain::memory::{FakeEmbedder, InMemoryStore};

    fn objects() -> Vec<(ObjectId, String)> {
        vec![
            (ObjectId(1), KIND_BED.to_string()),
            (ObjectId(4), KIND_TABLE.to_string()),
            (ObjectId(6), KIND_WORKBENCH.to_string()),
            (ObjectId(9), KIND_SOFA.to_string()),
            (ObjectId(2), "object/lamp".to_string()),
        ]
    }

    fn wiring() -> (Arc<dyn MemoryStore>, Arc<dyn Embedder>) {
        (Arc::new(InMemoryStore::new()), Arc::new(FakeEmbedder))
    }

    fn brains(agents: &[(EntId, String)]) -> Brains {
        let (store, embedder) = wiring();
        let mut b = Brains::new();
        b.rebuild(agents, &objects(), &store, &embedder);
        b
    }

    fn roster() -> Vec<(EntId, String)> {
        vec![
            (EntId(1), "omni".to_string()),
            (EntId(2), "worker".to_string()),
        ]
    }

    #[test]
    fn every_decision_crosses_the_crate_boundary_unchanged() {
        let cases = [
            (
                BrainDecision::GoTo(BrainTile::new(3, 4)),
                Decision::GoTo(Tile::new(3, 4)),
            ),
            (
                BrainDecision::Sit(BrainObject(7)),
                Decision::Sit(ObjectId(7)),
            ),
            (BrainDecision::Sleep, Decision::Sleep),
            (
                BrainDecision::Work(BrainObject(6)),
                Decision::Work(ObjectId(6)),
            ),
            (BrainDecision::Say("oi".into()), Decision::Say("oi".into())),
            (BrainDecision::Wave(BrainEnt(2)), Decision::Wave(EntId(2))),
            (BrainDecision::Idle, Decision::Idle),
        ];
        for (from, want) in cases {
            // The tag is the wire byte both crates agree on; asserting it here
            // is what catches a variant added to one side only.
            assert_eq!(from.tag(), want.tag(), "{from:?}");
            assert_eq!(to_world_decision(&from), want, "{from:?}");
        }
    }

    #[test]
    fn every_world_event_crosses_back_with_its_tag() {
        let cases = [
            WorldEvent::Arrived {
                ent: EntId(1),
                at: Tile::new(2, 3),
            },
            WorldEvent::StartedAnim {
                ent: EntId(1),
                anim: 2,
                dir: 3,
            },
            WorldEvent::Said {
                ent: EntId(1),
                text: "hi".into(),
            },
            WorldEvent::Interacted {
                ent: EntId(1),
                object: ObjectId(6),
            },
            WorldEvent::Slept { ent: EntId(1) },
            WorldEvent::Woke { ent: EntId(1) },
            WorldEvent::Yawned { ent: EntId(1) },
            WorldEvent::Spawned {
                ent: EntId(1),
                at: Tile::new(0, 1),
            },
            WorldEvent::Despawned { ent: EntId(1) },
            WorldEvent::ObjectPlaced {
                object: ObjectId(3),
                at: Tile::new(4, 4),
            },
            WorldEvent::ObjectRemoved {
                object: ObjectId(3),
            },
            WorldEvent::CaughtUp { ticks: 9 },
            WorldEvent::Rejected {
                ent: EntId(1),
                code: "ERR_WORLD_NO_PATH".into(),
            },
        ];
        for ev in cases {
            assert_eq!(
                to_brain_event(&ev).tag(),
                Some(ev.tag()),
                "the mirror and the crate disagree about {ev:?}"
            );
        }
    }

    #[test]
    fn the_furniture_of_a_day_comes_from_the_map() {
        let p = places_from_objects(&objects()).unwrap();
        assert_eq!(p.bed, BrainObject(1));
        assert_eq!(p.workbench, BrainObject(6));
        assert_eq!(p.table, BrainObject(4));
        assert_eq!(p.sofa, BrainObject(9));
        // No bed: the agenda still names a real object instead of id 0.
        let bare = places_from_objects(&[(ObjectId(6), KIND_WORKBENCH.into())]).unwrap();
        assert_eq!(bare.bed, BrainObject(6));
        // Nowhere to work at all: no brain rather than a plan about nothing.
        assert!(places_from_objects(&[(ObjectId(2), "object/lamp".into())]).is_none());
    }

    #[test]
    fn a_house_gets_one_brain_per_agent_and_keeps_them_across_rebuilds() {
        let (store, embedder) = wiring();
        let mut b = Brains::new();
        assert!(b.is_empty(), "nothing is allocated before a world exists");
        b.rebuild(&roster(), &objects(), &store, &embedder);
        assert_eq!(b.len(), 2);
        let first = b.get(EntId(1)).unwrap();
        b.rebuild(&roster(), &objects(), &store, &embedder);
        assert!(
            Arc::ptr_eq(&first, &b.get(EntId(1)).unwrap()),
            "a rebuild must not throw away a day of memories"
        );
        // Somebody left the house.
        b.rebuild(&roster()[..1], &objects(), &store, &embedder);
        assert_eq!(b.len(), 1);
        assert!(b.get(EntId(2)).is_none());
        b.clear();
        assert!(b.is_empty());
    }

    #[test]
    fn the_agenda_answers_without_a_single_model_call() {
        let b = brains(&roster());
        let mut inputs = 0;
        for minute in 0..1440u64 {
            inputs += b.tick_hints(minute * 6).len();
        }
        assert!(
            inputs >= 10,
            "two agents, a handful of changes each: {inputs}"
        );
        assert_eq!(b.model_calls(), 0, "the tick never calls a model");
    }

    #[test]
    fn what_happens_in_the_house_is_remembered_by_the_others() {
        let b = brains(&roster());
        let kept = b.observe(
            100,
            &[
                WorldEvent::Said {
                    ent: EntId(2),
                    text: "the radio is broken".into(),
                },
                // Noise: never a memory, for anybody.
                WorldEvent::StartedAnim {
                    ent: EntId(2),
                    anim: 1,
                    dir: 0,
                },
            ],
        );
        assert_eq!(kept, 2, "both agents remember the line (the speaker too)");
        assert_eq!(b.model_calls(), 0);
    }

    #[test]
    fn a_bus_event_is_remembered_but_does_not_move_anybody() {
        let b = brains(&roster());
        b.observe_bus(
            10,
            &BusEvent::ToolCalled {
                agent: "worker".into(),
                tool: "yt-dlp".into(),
                ok: true,
                ms: 12,
            },
        );
        // `bus_input` is what moves the body; the brain only learned about it.
        assert_eq!(b.model_calls(), 0);
        let worker = b.get(EntId(2)).unwrap();
        assert_eq!(worker.queue_len(), 1);
        assert_eq!(
            b.get(EntId(1)).unwrap().queue_len(),
            0,
            "not omni's business"
        );
    }

    #[test]
    fn energy_yawns_on_the_way_down_and_dawns_on_the_way_up() {
        let mut b = brains(&roster());
        assert!(b.set_energy(EntId(1), 200).is_empty());
        let tired = b.set_energy(EntId(1), 60);
        assert_eq!(tired.len(), 1, "the agent says something about being tired");
        assert!(matches!(
            &tired[0],
            Input::Decision { ent, decision: Decision::Say(_) } if *ent == EntId(1)
        ));
        // Same number again: nothing at all, not even a lookup.
        b.set_energy(EntId(1), 60);
        assert!(b.set_energy(EntId(1), 60).is_empty());

        let before = b.get(EntId(1)).unwrap().queue_len();
        b.set_energy(EntId(1), 0);
        b.set_energy(EntId(1), 255);
        assert_eq!(
            b.get(EntId(1)).unwrap().queue_len(),
            before + 1,
            "the window renewing is an event the agent remembers"
        );
        // An entity with no brain is not an error, it is just nobody.
        assert!(b.set_energy(EntId(99), 10).is_empty());
    }
}
