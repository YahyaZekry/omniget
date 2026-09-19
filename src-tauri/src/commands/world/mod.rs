//! Tauri bridge of the world (phase 7). Owned by f7-world-bridge.
//!
//! The crate `omniget-world` owns the simulation and knows nothing about
//! threads, clocks, channels or files. This module is everything it does not
//! know:
//!
//! - [`session`] — the ten `world_*` commands and the lazy `WorldManager`.
//! - [`brain`] — the conversion between `core::llm::brain`'s mirror types and
//!   the crate's, and the one [`Brain`](omniget_core::core::llm::brain::Brain)
//!   per agent that the tick thread feeds.
//! - [`brain_store`] — those memories in `<app_data>/world/memory.sqlite`.
//! - [`demo`] — `world_demo`: scripted jobs that put the residents to work
//!   without a model.
//! - [`tick`] — the sleep-state policy and the bounded outbound queue.
//! - [`input`] — JSON in, `Input` out; the bus and the quota windows in, `Input`
//!   out.
//! - [`save`] — `<app_data>/world/save.bin`, and putting a `World` back
//!   together from it.
//! - [`tier`] — `<app_data>/world/profile.json`: what the calibration measured
//!   and what the user pinned.
//!
//! Error codes are stable strings the route maps to a message, in the mould of
//! `StreamError::code()`. Codes that come out of the simulation itself
//! (`ERR_WORLD_MAP_INVALID`, `ERR_WORLD_TICK_TOO_OLD`, `ERR_WORLD_DIFF_GAP`, …)
//! are the crate's own and pass through untouched.

pub mod brain;
pub mod brain_store;
pub mod demo;
pub mod house;
pub mod input;
pub mod save;
pub mod session;
pub mod tick;
pub mod tier;

/// No world exists yet: the route has to offer "create a house".
pub const ERR_NO_WORLD: &str = "ERR_WORLD_NONE";
/// `settings.world.enabled` is off. The world is opt-in and off by default, and
/// a disabled world is not a world that is merely hidden: nothing is built, no
/// map is read, no thread runs and no brain exists. Every command that would
/// have to allocate something answers with this instead.
pub const ERR_WORLD_DISABLED: &str = "ERR_WORLD_DISABLED";
/// A world already exists and `world_create` will not overwrite it.
pub const ERR_WORLD_EXISTS: &str = "ERR_WORLD_EXISTS";
/// The house id is not a plain identifier.
pub const ERR_BAD_HOUSE: &str = "ERR_WORLD_BAD_HOUSE";
/// The house JSON is nowhere to be found.
pub const ERR_NO_MAP: &str = "ERR_WORLD_MAP_NOT_FOUND";
/// There is no application data directory to keep a world in.
pub const ERR_NO_DATA_DIR: &str = "ERR_WORLD_NO_DATA_DIR";

/// Emitted on every change of sleep state, with `{ "state": "active" | "dozing"
/// | "hibernating" }`. The only event the bridge has: everything else travels
/// on the channel.
pub const EVENT_SLEEP_STATE: &str = "world://sleep-state";
