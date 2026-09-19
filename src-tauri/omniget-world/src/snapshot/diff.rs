//! Diffs: what changed between two ticks, filtered by interest.
//!
//! At 10 Hz with eight agents walking the budget is 2 KB. A delta for a
//! walking agent is an id, a one-byte field mask and three zigzag varints —
//! nine bytes or so — which is why this format exists instead of JSON.
//!
//! # Body layout (kind = 2)
//!
//! Header from [`super::binary`], then:
//!
//! ```text
//! varint  from tick
//! varint  to tick
//! varint  map_hash
//! varint  rng_state
//! u8      sleep_state
//! table   strings
//! varint  n_entity_deltas       ... ascending EntId
//!   varint  id
//!   u8      field mask (see the FIELD_* constants)
//!   ... the present fields, in ascending bit order:
//!   bit0 POS       zigzag x, y, z
//!   bit1 DIR       u8
//!   bit2 ANIM      u8
//!   bit3 ENERGY    u8
//!   bit4 ACT       u8 tag, varint arg
//!   bit5 SPAWNED   varint name index
//!   bit6 DESPAWNED (no payload; no other bit may be set with it)
//! varint  n_object_deltas       ... ascending ObjectId
//!   varint  id
//!   u8      op: 0 removed, 1 placed or moved
//!   op 1:   varint kind index, zigzag tile x, tile y, u8 dir, u8 flags,
//!           varint height, u8 footprint w, u8 footprint d,
//!           varint slot index + 1
//! varint  n_events
//!   u8      tag, then the payload listed on WorldEvent
//! ```
//!
//! A spawn is `SPAWNED | POS | DIR | ANIM | ENERGY | ACT`: a full record, so a
//! client that joins mid-stream needs no separate message.

use crate::ents::agent::{Activity, ANIM_COUNT};
use crate::ents::id::{EntId, ObjectId};
use crate::error::{Result, WorldError};
use crate::fixed::Fixed;
use crate::map::Tile;
use crate::snapshot::binary::{Reader, StringTable, Writer, KIND_DIFF, MAX_ITEMS};
use crate::snapshot::{AgentState, ObjectState};

pub const FIELD_POS: u8 = 1 << 0;
pub const FIELD_DIR: u8 = 1 << 1;
pub const FIELD_ANIM: u8 = 1 << 2;
pub const FIELD_ENERGY: u8 = 1 << 3;
pub const FIELD_ACT: u8 = 1 << 4;
pub const FIELD_SPAWNED: u8 = 1 << 5;
pub const FIELD_DESPAWNED: u8 = 1 << 6;
/// Everything a spawn carries.
pub const FIELD_FULL: u8 =
    FIELD_POS | FIELD_DIR | FIELD_ANIM | FIELD_ENERGY | FIELD_ACT | FIELD_SPAWNED;

/// What changed about one agent.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct EntDelta {
    pub id: EntId,
    pub mask: u8,
    pub name: Option<String>,
    pub x: Fixed,
    pub y: Fixed,
    pub z: Fixed,
    pub dir: u8,
    pub anim: u8,
    pub energy: u8,
    pub activity: Activity,
}

impl EntDelta {
    pub fn is_despawn(&self) -> bool {
        self.mask & FIELD_DESPAWNED != 0
    }

    pub fn is_spawn(&self) -> bool {
        self.mask & FIELD_SPAWNED != 0
    }

    /// Everything about an agent that just appeared.
    pub fn spawned(a: &AgentState) -> EntDelta {
        EntDelta {
            id: a.id,
            mask: FIELD_FULL,
            name: Some(a.name.clone()),
            x: a.x,
            y: a.y,
            z: a.z,
            dir: a.dir,
            anim: a.anim,
            energy: a.energy,
            activity: a.activity,
        }
    }

    pub fn despawned(id: EntId) -> EntDelta {
        EntDelta {
            id,
            mask: FIELD_DESPAWNED,
            ..EntDelta::default()
        }
    }

    /// The fields in which `now` differs from `was`, or `None` when nothing
    /// changed and the agent should not be on the wire at all.
    pub fn between(was: &AgentState, now: &AgentState) -> Option<EntDelta> {
        let mut mask = 0u8;
        if was.x != now.x || was.y != now.y || was.z != now.z {
            mask |= FIELD_POS;
        }
        if was.dir != now.dir {
            mask |= FIELD_DIR;
        }
        if was.anim != now.anim {
            mask |= FIELD_ANIM;
        }
        if was.energy != now.energy {
            mask |= FIELD_ENERGY;
        }
        if was.activity != now.activity {
            mask |= FIELD_ACT;
        }
        if mask == 0 {
            return None;
        }
        // Only the fields the mask claims are filled in. A field the mask does
        // not mention is left at its default, so a delta that came off the
        // wire compares equal to the one that went on to it.
        let mut d = EntDelta {
            id: now.id,
            mask,
            ..EntDelta::default()
        };
        if mask & FIELD_POS != 0 {
            d.x = now.x;
            d.y = now.y;
            d.z = now.z;
        }
        if mask & FIELD_DIR != 0 {
            d.dir = now.dir;
        }
        if mask & FIELD_ANIM != 0 {
            d.anim = now.anim;
        }
        if mask & FIELD_ENERGY != 0 {
            d.energy = now.energy;
        }
        if mask & FIELD_ACT != 0 {
            d.activity = now.activity;
        }
        Some(d)
    }

    /// Fold this delta into a state record.
    pub fn apply_to(&self, a: &mut AgentState) {
        if self.mask & FIELD_POS != 0 {
            a.x = self.x;
            a.y = self.y;
            a.z = self.z;
        }
        if self.mask & FIELD_DIR != 0 {
            a.dir = self.dir;
        }
        if self.mask & FIELD_ANIM != 0 {
            a.anim = self.anim;
        }
        if self.mask & FIELD_ENERGY != 0 {
            a.energy = self.energy;
        }
        if self.mask & FIELD_ACT != 0 {
            a.activity = self.activity;
        }
        if let Some(n) = &self.name {
            a.name = n.clone();
        }
    }

    /// The `AgentState` a spawn delta describes.
    pub fn to_state(&self) -> AgentState {
        AgentState {
            id: self.id,
            name: self.name.clone().unwrap_or_default(),
            x: self.x,
            y: self.y,
            z: self.z,
            dir: self.dir,
            anim: self.anim,
            energy: self.energy,
            activity: self.activity,
        }
    }
}

/// What happened to one object.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ObjDelta {
    Removed(ObjectId),
    Placed(ObjectState),
}

impl ObjDelta {
    pub fn id(&self) -> ObjectId {
        match self {
            ObjDelta::Removed(id) => *id,
            ObjDelta::Placed(o) => o.id,
        }
    }
}

/// Something that happened during the ticks the diff covers. Events are for
/// things a state comparison cannot show — a word spoken, a yawn, a catch-up —
/// not for things the deltas already say.
#[derive(Clone, PartialEq, Eq, Debug)]
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
    /// Time was skipped rather than simulated.
    CaughtUp {
        ticks: u64,
    },
    /// A decision was refused, with the stable error code. The UI shows it in
    /// the agent's log instead of the world silently doing nothing.
    Rejected {
        ent: EntId,
        code: String,
    },
}

impl WorldEvent {
    pub const fn tag(&self) -> u8 {
        match self {
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
        }
    }

    /// The entity an event is about, when it is about one.
    pub fn ent(&self) -> Option<EntId> {
        match self {
            WorldEvent::Arrived { ent, .. }
            | WorldEvent::StartedAnim { ent, .. }
            | WorldEvent::Said { ent, .. }
            | WorldEvent::Interacted { ent, .. }
            | WorldEvent::Slept { ent }
            | WorldEvent::Woke { ent }
            | WorldEvent::Yawned { ent }
            | WorldEvent::Spawned { ent, .. }
            | WorldEvent::Despawned { ent }
            | WorldEvent::Rejected { ent, .. } => Some(*ent),
            _ => None,
        }
    }

    /// The tile an event happened on, when it has one, for interest filtering.
    pub fn at(&self) -> Option<Tile> {
        match self {
            WorldEvent::Arrived { at, .. }
            | WorldEvent::Spawned { at, .. }
            | WorldEvent::ObjectPlaced { at, .. } => Some(*at),
            _ => None,
        }
    }
}

/// The change between two ticks.
///
/// `rng_state` and `sleep` ride along so that a replica that applies every
/// diff is not merely similar to the authority but identical to it, down to
/// the next random number it will draw. That is what lets the room server and
/// the client run the same crate.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Diff {
    pub from: u64,
    pub to: u64,
    pub map_hash: u64,
    pub rng_state: u64,
    pub sleep: crate::sim::sleep::SleepState,
    pub ents: Vec<EntDelta>,
    pub objects: Vec<ObjDelta>,
    pub events: Vec<WorldEvent>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.ents.is_empty() && self.objects.is_empty() && self.events.is_empty()
    }

    pub fn encode(&self) -> Vec<u8> {
        self.encode_into(Writer::new())
    }

    pub fn encode_with(&self, buf: Vec<u8>) -> Vec<u8> {
        self.encode_into(Writer::with_buffer(buf))
    }

    fn encode_into(&self, mut w: Writer) -> Vec<u8> {
        let mut strings = StringTable::new();
        let mut body = Writer::new();

        body.varint(self.ents.len() as u64);
        for d in &self.ents {
            body.varint(d.id.0 as u64);
            body.u8(d.mask);
            if d.mask & FIELD_POS != 0 {
                body.zigzag(d.x.0);
                body.zigzag(d.y.0);
                body.zigzag(d.z.0);
            }
            if d.mask & FIELD_DIR != 0 {
                body.u8(d.dir);
            }
            if d.mask & FIELD_ANIM != 0 {
                body.u8(d.anim);
            }
            if d.mask & FIELD_ENERGY != 0 {
                body.u8(d.energy);
            }
            if d.mask & FIELD_ACT != 0 {
                body.u8(d.activity.tag());
                body.varint(d.activity.arg() as u64);
            }
            if d.mask & FIELD_SPAWNED != 0 {
                let idx = strings.intern(d.name.as_deref().unwrap_or(""));
                body.varint(idx as u64);
            }
        }

        body.varint(self.objects.len() as u64);
        for d in &self.objects {
            body.varint(d.id().0 as u64);
            match d {
                ObjDelta::Removed(_) => body.u8(0),
                ObjDelta::Placed(o) => {
                    body.u8(1);
                    let kind = strings.intern(&o.kind);
                    let slot = o.slot.as_ref().map(|s| strings.intern(s) + 1).unwrap_or(0);
                    body.varint(kind as u64);
                    body.zigzag(o.tile.x);
                    body.zigzag(o.tile.y);
                    body.u8(o.dir);
                    body.u8(o.walkable as u8);
                    body.varint(o.height as u64);
                    body.u8(o.footprint[0]);
                    body.u8(o.footprint[1]);
                    body.varint(slot as u64);
                }
            }
        }

        body.varint(self.events.len() as u64);
        for e in &self.events {
            body.u8(e.tag());
            match e {
                WorldEvent::Arrived { ent, at } | WorldEvent::Spawned { ent, at } => {
                    body.varint(ent.0 as u64);
                    body.zigzag(at.x);
                    body.zigzag(at.y);
                }
                WorldEvent::StartedAnim { ent, anim, dir } => {
                    body.varint(ent.0 as u64);
                    body.u8(*anim);
                    body.u8(*dir);
                }
                WorldEvent::Said { ent, text } => {
                    body.varint(ent.0 as u64);
                    body.varint(strings.intern(text) as u64);
                }
                WorldEvent::Rejected { ent, code } => {
                    body.varint(ent.0 as u64);
                    body.varint(strings.intern(code) as u64);
                }
                WorldEvent::Interacted { ent, object } => {
                    body.varint(ent.0 as u64);
                    body.varint(object.0 as u64);
                }
                WorldEvent::Slept { ent }
                | WorldEvent::Woke { ent }
                | WorldEvent::Yawned { ent }
                | WorldEvent::Despawned { ent } => body.varint(ent.0 as u64),
                WorldEvent::ObjectPlaced { object, at } => {
                    body.varint(object.0 as u64);
                    body.zigzag(at.x);
                    body.zigzag(at.y);
                }
                WorldEvent::ObjectRemoved { object } => body.varint(object.0 as u64),
                WorldEvent::CaughtUp { ticks } => body.varint(*ticks),
            }
        }

        w.header(KIND_DIFF);
        w.varint(self.from);
        w.varint(self.to);
        w.varint(self.map_hash);
        w.varint(self.rng_state);
        w.u8(self.sleep.tag());
        strings.write(&mut w);
        let mut out = w.finish();
        out.extend_from_slice(body.as_slice());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Diff> {
        let mut r = Reader::new(buf);
        let kind = r.header()?;
        if kind != KIND_DIFF {
            return Err(WorldError::BadKind(kind));
        }
        let from = r.varint()?;
        let to = r.varint()?;
        let map_hash = r.varint()?;
        let rng_state = r.varint()?;
        let sleep_tag = r.u8()?;
        let sleep =
            crate::sim::sleep::SleepState::from_tag(sleep_tag).ok_or(WorldError::BadTag {
                what: "sleep state",
                tag: sleep_tag,
            })?;
        let strings = StringTable::read(&mut r)?;

        let n = r.count(MAX_ITEMS)?;
        let mut ents = Vec::with_capacity(n);
        for _ in 0..n {
            let id = EntId(r.varint()? as u32);
            let mask = r.u8()?;
            if mask & FIELD_DESPAWNED != 0 && mask != FIELD_DESPAWNED {
                return Err(WorldError::BadTag {
                    what: "entity delta mask",
                    tag: mask,
                });
            }
            let mut d = EntDelta {
                id,
                mask,
                ..EntDelta::default()
            };
            if mask & FIELD_POS != 0 {
                d.x = Fixed(r.zigzag()?);
                d.y = Fixed(r.zigzag()?);
                d.z = Fixed(r.zigzag()?);
            }
            if mask & FIELD_DIR != 0 {
                d.dir = r.u8()?;
            }
            if mask & FIELD_ANIM != 0 {
                d.anim = r.u8()?;
                if d.anim >= ANIM_COUNT {
                    return Err(WorldError::BadTag {
                        what: "anim",
                        tag: d.anim,
                    });
                }
            }
            if mask & FIELD_ENERGY != 0 {
                d.energy = r.u8()?;
            }
            if mask & FIELD_ACT != 0 {
                let tag = r.u8()?;
                let arg = r.varint()? as u32;
                d.activity = Activity::from_parts(tag, arg).ok_or(WorldError::BadTag {
                    what: "activity",
                    tag,
                })?;
            }
            if mask & FIELD_SPAWNED != 0 {
                d.name = Some(strings.get(r.varint()? as u32)?.to_string());
            }
            ents.push(d);
        }

        let n = r.count(MAX_ITEMS)?;
        let mut objects = Vec::with_capacity(n);
        for _ in 0..n {
            let id = ObjectId(r.varint()? as u32);
            let op = r.u8()?;
            match op {
                0 => objects.push(ObjDelta::Removed(id)),
                1 => {
                    let kind = strings.get(r.varint()? as u32)?.to_string();
                    let tile = Tile::new(r.zigzag()?, r.zigzag()?);
                    let dir = r.u8()?;
                    let walkable = r.u8()? != 0;
                    let height = r.varint()? as u16;
                    let footprint = [r.u8()?, r.u8()?];
                    let slot_idx = r.varint()? as u32;
                    let slot = if slot_idx == 0 {
                        None
                    } else {
                        Some(strings.get(slot_idx - 1)?.to_string())
                    };
                    objects.push(ObjDelta::Placed(ObjectState {
                        id,
                        kind,
                        tile,
                        dir,
                        walkable,
                        height,
                        footprint,
                        slot,
                    }));
                }
                other => {
                    return Err(WorldError::BadTag {
                        what: "object delta op",
                        tag: other,
                    })
                }
            }
        }

        let n = r.count(MAX_ITEMS)?;
        let mut events = Vec::with_capacity(n);
        for _ in 0..n {
            let tag = r.u8()?;
            let e = match tag {
                0 => WorldEvent::Arrived {
                    ent: EntId(r.varint()? as u32),
                    at: Tile::new(r.zigzag()?, r.zigzag()?),
                },
                1 => WorldEvent::StartedAnim {
                    ent: EntId(r.varint()? as u32),
                    anim: r.u8()?,
                    dir: r.u8()?,
                },
                2 => WorldEvent::Said {
                    ent: EntId(r.varint()? as u32),
                    text: strings.get(r.varint()? as u32)?.to_string(),
                },
                3 => WorldEvent::Interacted {
                    ent: EntId(r.varint()? as u32),
                    object: ObjectId(r.varint()? as u32),
                },
                4 => WorldEvent::Slept {
                    ent: EntId(r.varint()? as u32),
                },
                5 => WorldEvent::Woke {
                    ent: EntId(r.varint()? as u32),
                },
                6 => WorldEvent::Yawned {
                    ent: EntId(r.varint()? as u32),
                },
                7 => WorldEvent::Spawned {
                    ent: EntId(r.varint()? as u32),
                    at: Tile::new(r.zigzag()?, r.zigzag()?),
                },
                8 => WorldEvent::Despawned {
                    ent: EntId(r.varint()? as u32),
                },
                9 => WorldEvent::ObjectPlaced {
                    object: ObjectId(r.varint()? as u32),
                    at: Tile::new(r.zigzag()?, r.zigzag()?),
                },
                10 => WorldEvent::ObjectRemoved {
                    object: ObjectId(r.varint()? as u32),
                },
                11 => WorldEvent::CaughtUp { ticks: r.varint()? },
                12 => WorldEvent::Rejected {
                    ent: EntId(r.varint()? as u32),
                    code: strings.get(r.varint()? as u32)?.to_string(),
                },
                other => {
                    return Err(WorldError::BadTag {
                        what: "world event",
                        tag: other,
                    })
                }
            };
            events.push(e);
        }
        r.finish()?;
        Ok(Diff {
            from,
            to,
            map_hash,
            rng_state,
            sleep,
            ents,
            objects,
            events,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: u32) -> AgentState {
        AgentState {
            id: EntId(id),
            name: format!("a{id}"),
            x: Fixed(100),
            y: Fixed(200),
            z: Fixed::ZERO,
            dir: 0,
            anim: 0,
            energy: 255,
            activity: Activity::Idle,
        }
    }

    fn sample() -> Diff {
        Diff {
            from: 10,
            to: 11,
            map_hash: 0xabc,
            rng_state: 0x1234_5678_9abc,
            sleep: crate::sim::sleep::SleepState::Dozing,
            ents: vec![
                EntDelta::between(
                    &agent(1),
                    &AgentState {
                        x: Fixed(-300),
                        dir: 4,
                        ..agent(1)
                    },
                )
                .unwrap(),
                EntDelta::spawned(&agent(2)),
                EntDelta::despawned(EntId(3)),
            ],
            objects: vec![
                ObjDelta::Removed(ObjectId(5)),
                ObjDelta::Placed(ObjectState {
                    id: ObjectId(6),
                    kind: "object/chair".into(),
                    tile: Tile::new(-3, 9),
                    dir: 1,
                    walkable: false,
                    height: 16,
                    footprint: [1, 1],
                    slot: Some("corner".into()),
                }),
            ],
            events: vec![
                WorldEvent::Arrived {
                    ent: EntId(1),
                    at: Tile::new(2, 3),
                },
                WorldEvent::StartedAnim {
                    ent: EntId(1),
                    anim: 1,
                    dir: 2,
                },
                WorldEvent::Said {
                    ent: EntId(2),
                    text: "olá".into(),
                },
                WorldEvent::Interacted {
                    ent: EntId(2),
                    object: ObjectId(6),
                },
                WorldEvent::Slept { ent: EntId(1) },
                WorldEvent::Woke { ent: EntId(1) },
                WorldEvent::Yawned { ent: EntId(1) },
                WorldEvent::Spawned {
                    ent: EntId(2),
                    at: Tile::new(0, 0),
                },
                WorldEvent::Despawned { ent: EntId(3) },
                WorldEvent::ObjectPlaced {
                    object: ObjectId(6),
                    at: Tile::new(-3, 9),
                },
                WorldEvent::ObjectRemoved {
                    object: ObjectId(5),
                },
                WorldEvent::CaughtUp { ticks: 288_000 },
                WorldEvent::Rejected {
                    ent: EntId(1),
                    code: "ERR_WORLD_NO_PATH".into(),
                },
            ],
        }
    }

    #[test]
    fn diff_round_trips_with_every_variant() {
        let d = sample();
        assert_eq!(Diff::decode(&d.encode()).unwrap(), d);
    }

    #[test]
    fn every_event_variant_is_covered_by_the_sample() {
        let tags: std::collections::BTreeSet<u8> =
            sample().events.iter().map(|e| e.tag()).collect();
        assert_eq!(tags.len(), 13, "one sample per tag");
        assert_eq!(*tags.iter().next_back().unwrap(), 12);
    }

    #[test]
    fn an_empty_diff_is_tiny() {
        let d = Diff::default();
        let bytes = d.encode();
        assert!(bytes.len() <= 16, "{} bytes", bytes.len());
        assert_eq!(Diff::decode(&bytes).unwrap(), d);
        assert!(d.is_empty());
    }

    #[test]
    fn between_reports_only_what_moved() {
        let a = agent(1);
        assert_eq!(EntDelta::between(&a, &a), None);
        let mut b = a.clone();
        b.energy = 3;
        let d = EntDelta::between(&a, &b).unwrap();
        assert_eq!(d.mask, FIELD_ENERGY);
        assert_eq!(d.energy, 3);
        let mut c = a.clone();
        c.activity = Activity::Sleeping;
        assert_eq!(EntDelta::between(&a, &c).unwrap().mask, FIELD_ACT);
    }

    #[test]
    fn a_delta_folds_back_into_the_state_it_came_from() {
        let was = agent(1);
        let now = AgentState {
            x: Fixed(999),
            dir: 5,
            anim: 1,
            energy: 7,
            activity: Activity::Walking,
            ..agent(1)
        };
        let mut rebuilt = was.clone();
        EntDelta::between(&was, &now)
            .unwrap()
            .apply_to(&mut rebuilt);
        assert_eq!(rebuilt, now);
    }

    #[test]
    fn a_spawn_carries_the_whole_agent() {
        let a = agent(4);
        let d = EntDelta::spawned(&a);
        assert!(d.is_spawn());
        assert!(!d.is_despawn());
        assert_eq!(d.to_state(), a);
        let back = Diff::decode(
            &Diff {
                ents: vec![d],
                ..Diff::default()
            }
            .encode(),
        )
        .unwrap();
        assert_eq!(back.ents[0].to_state(), a);
    }

    #[test]
    fn a_despawn_may_not_carry_anything_else() {
        let mut d = Diff::default();
        d.ents.push(EntDelta {
            id: EntId(1),
            mask: FIELD_DESPAWNED | FIELD_POS,
            ..EntDelta::default()
        });
        assert!(matches!(
            Diff::decode(&d.encode()),
            Err(WorldError::BadTag { .. })
        ));
    }

    #[test]
    fn an_unknown_event_tag_is_refused() {
        let mut bytes = Diff {
            events: vec![WorldEvent::Slept { ent: EntId(1) }],
            ..Diff::default()
        }
        .encode();
        let n = bytes.len();
        bytes[n - 2] = 99;
        assert!(matches!(
            Diff::decode(&bytes),
            Err(WorldError::BadTag {
                what: "world event",
                ..
            })
        ));
    }

    #[test]
    fn a_snapshot_blob_is_refused_by_the_diff_decoder() {
        let s = crate::snapshot::Snapshot {
            tick: 0,
            seed: 0,
            rng_state: 0,
            map_hash: 0,
            sleep: crate::sim::sleep::SleepState::Active,
            agents: vec![],
            objects: vec![],
        };
        assert!(matches!(
            Diff::decode(&s.encode()),
            Err(WorldError::BadKind(_))
        ));
    }

    #[test]
    fn corrupt_diffs_are_errors_not_panics() {
        let bytes = sample().encode();
        for cut in 0..bytes.len() {
            assert!(Diff::decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn eight_walking_agents_cost_far_less_than_two_kilobytes() {
        let mut d = Diff {
            from: 100,
            to: 101,
            ..Diff::default()
        };
        for i in 1..=8u32 {
            let was = agent(i);
            let now = AgentState {
                x: Fixed(was.x.0 + 38),
                y: Fixed(was.y.0 - 12),
                dir: 3,
                anim: 1,
                ..was.clone()
            };
            d.ents.push(EntDelta::between(&was, &now).unwrap());
        }
        let n = d.encode().len();
        assert!(n < 2048, "{n} bytes");
        assert!(n < 200, "{n} bytes: a walking agent is about ten");
    }

    #[test]
    fn events_know_their_entity_and_their_tile() {
        let e = WorldEvent::Arrived {
            ent: EntId(2),
            at: Tile::new(1, 1),
        };
        assert_eq!(e.ent(), Some(EntId(2)));
        assert_eq!(e.at(), Some(Tile::new(1, 1)));
        let c = WorldEvent::CaughtUp { ticks: 1 };
        assert_eq!(c.ent(), None);
        assert_eq!(c.at(), None);
        assert_eq!(
            WorldEvent::ObjectPlaced {
                object: ObjectId(1),
                at: Tile::new(4, 4)
            }
            .at(),
            Some(Tile::new(4, 4))
        );
    }
}
