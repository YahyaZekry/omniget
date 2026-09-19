//! Snapshots: the whole observable state of the world in one blob.
//!
//! "Observable" is the load-bearing word. A snapshot carries what a renderer
//! or a visitor is allowed to see and nothing else: no paths, no routines, no
//! mailbox, no A* workspace. That is what makes `apply(diff_since(t))` equal
//! to `snapshot()` a testable statement instead of a wish.
//!
//! # Body layout (kind = 1)
//!
//! Header from [`binary`], then:
//!
//! ```text
//! varint  tick
//! varint  seed
//! varint  rng_state
//! varint  map_hash
//! u8      sleep_state (0 active, 1 dozing, 2 hibernating)
//! table   strings
//! varint  n_agents           ... ascending EntId
//!   varint  id
//!   varint  name index
//!   zigzag  x, y, z          ... Fixed raw, 1/256 tile
//!   u8      dir, anim, energy, activity tag
//!   varint  activity arg     ... ObjectId or EntId, 0 when the tag has none
//! varint  n_objects          ... ascending ObjectId
//!   varint  id
//!   varint  kind index
//!   zigzag  tile x, tile y
//!   u8      dir
//!   u8      flags            ... bit0 walkable
//!   varint  height           ... atlas pixels
//!   u8      footprint w, footprint d
//!   varint  slot index + 1   ... 0 means "in no slot"
//! ```

pub mod binary;
pub mod diff;
pub mod interest;

use crate::ents::agent::{Activity, ANIM_COUNT};
use crate::ents::id::{EntId, ObjectId};
use crate::error::{Result, WorldError};
use crate::fixed::Fixed;
use crate::map::Tile;
use crate::sim::sleep::SleepState;
use binary::{Reader, StringTable, Writer, KIND_SNAPSHOT, MAX_ITEMS};

pub use diff::{
    Diff, EntDelta, ObjDelta, WorldEvent, FIELD_ACT, FIELD_ANIM, FIELD_DESPAWNED, FIELD_DIR,
    FIELD_ENERGY, FIELD_POS, FIELD_SPAWNED,
};
pub use interest::Interest;

/// One agent as the outside world sees it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AgentState {
    pub id: EntId,
    pub name: String,
    pub x: Fixed,
    pub y: Fixed,
    pub z: Fixed,
    pub dir: u8,
    pub anim: u8,
    pub energy: u8,
    pub activity: Activity,
}

impl AgentState {
    pub fn tile(&self) -> Tile {
        Tile::new(self.x.floor_tile(), self.y.floor_tile())
    }
}

/// One placed object as the outside world sees it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ObjectState {
    pub id: ObjectId,
    pub kind: String,
    pub tile: Tile,
    pub dir: u8,
    pub walkable: bool,
    pub height: u16,
    pub footprint: [u8; 2],
    pub slot: Option<String>,
}

/// The observable state at one tick.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Snapshot {
    pub tick: u64,
    pub seed: u64,
    pub rng_state: u64,
    pub map_hash: u64,
    pub sleep: SleepState,
    /// Ascending by id.
    pub agents: Vec<AgentState>,
    /// Ascending by id.
    pub objects: Vec<ObjectState>,
}

impl Snapshot {
    pub fn encode(&self) -> Vec<u8> {
        self.encode_into(Writer::new())
    }

    /// Encode into a buffer the caller owns, so the tick thread can keep one
    /// around instead of allocating every time.
    pub fn encode_with(&self, buf: Vec<u8>) -> Vec<u8> {
        self.encode_into(Writer::with_buffer(buf))
    }

    fn encode_into(&self, mut w: Writer) -> Vec<u8> {
        let mut strings = StringTable::new();
        // The table has to be complete before it is written, so the records
        // are built into a scratch writer first and appended after.
        let mut body = Writer::new();
        body.varint(self.agents.len() as u64);
        for a in &self.agents {
            let name = strings.intern(&a.name);
            body.varint(a.id.0 as u64);
            body.varint(name as u64);
            body.zigzag(a.x.0);
            body.zigzag(a.y.0);
            body.zigzag(a.z.0);
            body.u8(a.dir);
            body.u8(a.anim);
            body.u8(a.energy);
            body.u8(a.activity.tag());
            body.varint(a.activity.arg() as u64);
        }
        body.varint(self.objects.len() as u64);
        for o in &self.objects {
            let kind = strings.intern(&o.kind);
            let slot = o.slot.as_ref().map(|s| strings.intern(s) + 1).unwrap_or(0);
            body.varint(o.id.0 as u64);
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

        w.header(KIND_SNAPSHOT);
        w.varint(self.tick);
        w.varint(self.seed);
        w.varint(self.rng_state);
        w.varint(self.map_hash);
        w.u8(self.sleep.tag());
        strings.write(&mut w);
        let mut out = w.finish();
        out.extend_from_slice(body.as_slice());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Snapshot> {
        let mut r = Reader::new(buf);
        let kind = r.header()?;
        if kind != KIND_SNAPSHOT {
            return Err(WorldError::BadKind(kind));
        }
        let tick = r.varint()?;
        let seed = r.varint()?;
        let rng_state = r.varint()?;
        let map_hash = r.varint()?;
        let sleep_tag = r.u8()?;
        let sleep = SleepState::from_tag(sleep_tag).ok_or(WorldError::BadTag {
            what: "sleep state",
            tag: sleep_tag,
        })?;
        let strings = StringTable::read(&mut r)?;

        let n = r.count(MAX_ITEMS)?;
        let mut agents = Vec::with_capacity(n);
        for _ in 0..n {
            let id = EntId(r.varint()? as u32);
            let name = strings.get(r.varint()? as u32)?.to_string();
            let x = Fixed(r.zigzag()?);
            let y = Fixed(r.zigzag()?);
            let z = Fixed(r.zigzag()?);
            let dir = r.u8()?;
            let anim = r.u8()?;
            let energy = r.u8()?;
            let act_tag = r.u8()?;
            let act_arg = r.varint()? as u32;
            if anim >= ANIM_COUNT {
                return Err(WorldError::BadTag {
                    what: "anim",
                    tag: anim,
                });
            }
            let activity = Activity::from_parts(act_tag, act_arg).ok_or(WorldError::BadTag {
                what: "activity",
                tag: act_tag,
            })?;
            agents.push(AgentState {
                id,
                name,
                x,
                y,
                z,
                dir,
                anim,
                energy,
                activity,
            });
        }

        let n = r.count(MAX_ITEMS)?;
        let mut objects = Vec::with_capacity(n);
        for _ in 0..n {
            let id = ObjectId(r.varint()? as u32);
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
            objects.push(ObjectState {
                id,
                kind,
                tile,
                dir,
                walkable,
                height,
                footprint,
                slot,
            });
        }
        r.finish()?;
        Ok(Snapshot {
            tick,
            seed,
            rng_state,
            map_hash,
            sleep,
            agents,
            objects,
        })
    }

    /// A 64-bit fingerprint of the observable state. The determinism tests
    /// compare this, and it is what `docs/bench/world-f7-tick.md` records, so
    /// three operating systems can be compared with one number.
    pub fn fingerprint(&self) -> u64 {
        let bytes = self.encode();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            tick: 1234,
            seed: 99,
            rng_state: 0xdead_beef,
            map_hash: 0x0123_4567_89ab_cdef,
            sleep: SleepState::Dozing,
            agents: vec![
                AgentState {
                    id: EntId(1),
                    name: "Omni".into(),
                    x: Fixed(640),
                    y: Fixed(-128),
                    z: Fixed::ZERO,
                    dir: 3,
                    anim: 1,
                    energy: 200,
                    activity: Activity::Walking,
                },
                AgentState {
                    id: EntId(2),
                    name: "Ada".into(),
                    x: Fixed(1),
                    y: Fixed(2),
                    z: Fixed::ONE,
                    dir: 0,
                    anim: 4,
                    energy: 10,
                    activity: Activity::Working(ObjectId(7)),
                },
            ],
            objects: vec![ObjectState {
                id: ObjectId(7),
                kind: "object/workbench".into(),
                tile: Tile::new(4, -4),
                dir: 2,
                walkable: false,
                height: 20,
                footprint: [2, 1],
                slot: Some("bench".into()),
            }],
        }
    }

    #[test]
    fn snapshot_round_trips() {
        let s = sample();
        let bytes = s.encode();
        assert_eq!(Snapshot::decode(&bytes).unwrap(), s);
    }

    #[test]
    fn the_blob_starts_with_the_documented_header() {
        let bytes = sample().encode();
        assert_eq!(&bytes[..4], b"OGW1");
        assert_eq!(bytes[4], KIND_SNAPSHOT);
        assert_eq!(bytes[5], binary::FORMAT_VERSION);
    }

    #[test]
    fn encoding_is_stable_byte_for_byte() {
        // Two encodes of the same state must be identical, or the fingerprint
        // and the cross-platform test mean nothing.
        assert_eq!(sample().encode(), sample().encode());
        assert_eq!(sample().fingerprint(), sample().fingerprint());
    }

    #[test]
    fn an_empty_world_still_encodes() {
        let s = Snapshot {
            tick: 0,
            seed: 0,
            rng_state: 0,
            map_hash: 0,
            sleep: SleepState::Active,
            agents: vec![],
            objects: vec![],
        };
        assert_eq!(Snapshot::decode(&s.encode()).unwrap(), s);
    }

    #[test]
    fn a_diff_blob_is_refused_by_the_snapshot_decoder() {
        let d = Diff::default();
        assert!(matches!(
            Snapshot::decode(&d.encode()),
            Err(WorldError::BadKind(_))
        ));
    }

    #[test]
    fn corrupt_blobs_are_errors_not_panics() {
        let bytes = sample().encode();
        for cut in 0..bytes.len() {
            assert!(Snapshot::decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut flipped = bytes.clone();
        let n = flipped.len();
        flipped[n - 1] ^= 0xff;
        let _ = Snapshot::decode(&flipped);
    }

    #[test]
    fn a_nonsense_activity_tag_is_refused() {
        let mut s = sample();
        s.agents.truncate(1);
        let mut bytes = s.encode();
        // The activity tag is the fourth of the four raw bytes of the record.
        let pos = bytes
            .windows(4)
            .position(|w| w == [3u8, 1, 200, 1])
            .expect("agent record");
        bytes[pos + 3] = 99;
        assert!(matches!(
            Snapshot::decode(&bytes),
            Err(WorldError::BadTag {
                what: "activity",
                ..
            })
        ));
    }

    #[test]
    fn names_are_shared_between_agents() {
        let mut s = sample();
        s.agents[1].name = "Omni".into();
        let shared = s.encode().len();
        let mut t = sample();
        t.agents[1].name = "Ada".into();
        assert!(shared < t.encode().len(), "the table deduplicates");
    }

    #[test]
    fn a_slotless_object_costs_one_byte_of_slot() {
        let mut a = sample();
        a.objects[0].slot = None;
        assert!(a.encode().len() < sample().encode().len());
        assert_eq!(Snapshot::decode(&a.encode()).unwrap().objects[0].slot, None);
    }

    #[test]
    fn twenty_four_agents_fit_in_a_kilobyte() {
        let mut s = sample();
        s.agents.clear();
        for i in 1..=24u32 {
            s.agents.push(AgentState {
                id: EntId(i),
                name: format!("agent-{i}"),
                x: Fixed(i as i32 * 256),
                y: Fixed(i as i32 * 256),
                z: Fixed::ZERO,
                dir: (i % 8) as u8,
                anim: 1,
                energy: 128,
                activity: Activity::Walking,
            });
        }
        let n = s.encode().len();
        assert!(n < 1024, "{n} bytes for a 24-agent snapshot");
    }
}
