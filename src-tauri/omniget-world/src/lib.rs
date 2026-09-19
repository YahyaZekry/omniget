//! Deterministic fixed-tick world simulation for OmniGet.
//!
//! No GPU, no Tauri, no threads, no clock, no network. The same crate runs
//! inside the app as the authority for the single-player house and, later,
//! inside the room server as the authority for a shared one. That is only
//! possible because everything here is a pure function of a seed, a map and a
//! list of inputs.
//!
//! # The contract
//!
//! ```no_run
//! use omniget_world::{Input, Interest, World};
//! # fn map() -> omniget_world::MapDef { unimplemented!() }
//! let mut w = World::new(42, map()).unwrap();
//! let report = w.step(&[Input::Tick]);   // one tick, 100 ms of world time
//! let snap = w.snapshot();               // the whole observable state
//! let diff = w.diff_since(report.tick - 1, &Interest::ALL).unwrap();
//! ```
//!
//! - **Tick**: fixed at 10 Hz ([`TICK_MS`]). `step()` never fails; a refused
//!   input becomes a [`WorldEvent::Rejected`] with a stable `ERR_*` code.
//! - **Determinism**: the same seed and the same inputs produce the same
//!   `snapshot()` on macOS, Linux and Windows. There is no `f32` and no `f64`
//!   in the state, positions are [`Fixed`] at 1/256 of a tile, randomness is
//!   our own xorshift, and every map in the state is a `BTreeMap`, never a
//!   `HashMap`.
//! - **`apply(diff_since(t))` equals `snapshot()`**: a replica that folds in
//!   every diff is identical to the authority, down to the next random number
//!   it will draw. `tests/determinism.rs` asserts both.
//! - **The model is never in the tick**: decisions arrive through
//!   [`Input::Decision`], land in a [`Mailbox`], and are read by the tick. The
//!   tick never calls out.
//! - **Energy is an input**, not a decision: it is the remaining quota of the
//!   account an agent uses (plan §9.1), pushed in with [`Input::SetEnergy`].
//!
//! # The binary format
//!
//! Snapshots and diffs are a format of our own — varints, zigzag and a string
//! table, versioned in a six-byte header — because the same bytes go over a
//! `tauri::ipc::Channel` at 10 Hz and, later, over a socket. It is documented
//! byte by byte in [`snapshot::binary`], [`snapshot`] and [`snapshot::diff`],
//! and the TypeScript decoder in `src/lib/world/` is written from those three
//! doc comments. `serde` is used for one thing only: the `MapDef` JSON.

pub mod ents;
pub mod error;
pub mod fixed;
pub mod map;
pub mod path;
pub mod rng;
pub mod sim;
pub mod snapshot;
pub mod world;

pub use ents::{Activity, EntId, ObjectId};
pub use error::{Rejection, Result, WorldError};
pub use fixed::Fixed;
pub use map::{Map, MapDef, Tile, TileDef, CHUNK_TILES, MAP_FORMAT_VERSION};
pub use path::{AStar, Grid};
pub use rng::Rng;
pub use sim::{Decision, Mailbox, Routine, RoutineEntry, SleepState, TICKS_PER_DAY, TICK_MS};
pub use snapshot::binary::{FORMAT_VERSION, MAGIC};
pub use snapshot::{AgentState, Diff, EntDelta, Interest, ObjectState, Snapshot, WorldEvent};
pub use world::{Input, StepReport, World};

/// Version of the wire format this build speaks, for a handshake.
pub const WIRE_VERSION: u8 = FORMAT_VERSION;
