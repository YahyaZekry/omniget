//! Every failure the world can report, with a stable `ERR_*` code the UI maps,
//! in the mould of `StreamError::code()`.

use std::fmt;

/// Stable error codes. Adding a variant is additive; renaming one is a break.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
pub enum WorldError {
    #[error("ERR_WORLD_BAD_MAGIC: not an OmniGet world blob")]
    BadMagic,
    #[error("ERR_WORLD_BAD_VERSION: blob format {got}, this build reads {want}")]
    BadVersion { got: u8, want: u8 },
    #[error("ERR_WORLD_BAD_KIND: blob kind {0}, expected snapshot(1) or diff(2)")]
    BadKind(u8),
    #[error("ERR_WORLD_TRUNCATED: blob ends after {at} bytes, {need} more expected")]
    Truncated { at: usize, need: usize },
    #[error("ERR_WORLD_VARINT: varint at {0} does not terminate within 10 bytes")]
    Varint(usize),
    #[error("ERR_WORLD_BAD_STRING: string {0} is not valid UTF-8 or is out of range")]
    BadString(usize),
    #[error("ERR_WORLD_BAD_TAG: unknown tag {tag} in {what}")]
    BadTag { what: &'static str, tag: u8 },
    #[error("ERR_WORLD_TRAILING: {0} bytes left after the blob")]
    Trailing(usize),
    #[error("ERR_WORLD_UNKNOWN_ENT: entity {0} does not exist")]
    UnknownEnt(u32),
    #[error("ERR_WORLD_UNKNOWN_OBJECT: object {0} does not exist")]
    UnknownObject(u32),
    #[error("ERR_WORLD_DUPLICATE_ENT: entity {0} already exists")]
    DuplicateEnt(u32),
    #[error("ERR_WORLD_OUT_OF_BOUNDS: tile ({x}, {y}) is outside the map")]
    OutOfBounds { x: i32, y: i32 },
    #[error("ERR_WORLD_NOT_WALKABLE: tile ({x}, {y}) cannot be stood on")]
    NotWalkable { x: i32, y: i32 },
    #[error("ERR_WORLD_NO_PATH: no route from ({fx}, {fy}) to ({tx}, {ty})")]
    NoPath { fx: i32, fy: i32, tx: i32, ty: i32 },
    #[error("ERR_WORLD_MAP_INVALID: {0}")]
    MapInvalid(String),
    #[error("ERR_WORLD_TICK_TOO_OLD: tick {asked} is older than the kept history ({oldest})")]
    TickTooOld { asked: u64, oldest: u64 },
    #[error("ERR_WORLD_DIFF_GAP: diff starts at {from} but the world is at {tick}")]
    DiffGap { from: u64, tick: u64 },
    #[error("ERR_WORLD_MAP_MISMATCH: blob was made for map {blob:#x}, this world is {world:#x}")]
    MapMismatch { blob: u64, world: u64 },
    #[error("ERR_WORLD_TOO_LONG: {what} is {got} long, the limit is {max}")]
    TooLong {
        what: &'static str,
        got: usize,
        max: usize,
    },
    #[error("ERR_WORLD_BAD_SLOT: slot {0} does not exist or refuses this object")]
    BadSlot(String),
}

impl WorldError {
    /// The `ERR_*` prefix alone, for the UI to switch on.
    pub fn code(&self) -> &'static str {
        match self {
            WorldError::BadMagic => "ERR_WORLD_BAD_MAGIC",
            WorldError::BadVersion { .. } => "ERR_WORLD_BAD_VERSION",
            WorldError::BadKind(_) => "ERR_WORLD_BAD_KIND",
            WorldError::Truncated { .. } => "ERR_WORLD_TRUNCATED",
            WorldError::Varint(_) => "ERR_WORLD_VARINT",
            WorldError::BadString(_) => "ERR_WORLD_BAD_STRING",
            WorldError::BadTag { .. } => "ERR_WORLD_BAD_TAG",
            WorldError::Trailing(_) => "ERR_WORLD_TRAILING",
            WorldError::UnknownEnt(_) => "ERR_WORLD_UNKNOWN_ENT",
            WorldError::UnknownObject(_) => "ERR_WORLD_UNKNOWN_OBJECT",
            WorldError::DuplicateEnt(_) => "ERR_WORLD_DUPLICATE_ENT",
            WorldError::OutOfBounds { .. } => "ERR_WORLD_OUT_OF_BOUNDS",
            WorldError::NotWalkable { .. } => "ERR_WORLD_NOT_WALKABLE",
            WorldError::NoPath { .. } => "ERR_WORLD_NO_PATH",
            WorldError::MapInvalid(_) => "ERR_WORLD_MAP_INVALID",
            WorldError::TickTooOld { .. } => "ERR_WORLD_TICK_TOO_OLD",
            WorldError::DiffGap { .. } => "ERR_WORLD_DIFF_GAP",
            WorldError::MapMismatch { .. } => "ERR_WORLD_MAP_MISMATCH",
            WorldError::TooLong { .. } => "ERR_WORLD_TOO_LONG",
            WorldError::BadSlot(_) => "ERR_WORLD_BAD_SLOT",
        }
    }
}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, WorldError>;

/// A rejected input, reported by `step()` instead of failing the whole tick:
/// one bad decision from a model must never stop the world.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rejection {
    pub ent: u32,
    pub error: WorldError,
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ent {}: {}", self.ent, self.error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_the_message_prefix() {
        let cases = [
            WorldError::BadMagic,
            WorldError::BadVersion { got: 2, want: 1 },
            WorldError::Truncated { at: 3, need: 4 },
            WorldError::NoPath {
                fx: 0,
                fy: 0,
                tx: 1,
                ty: 1,
            },
            WorldError::MapInvalid("x".into()),
            WorldError::BadSlot("s".into()),
        ];
        for c in cases {
            let msg = c.to_string();
            assert!(msg.starts_with(c.code()), "{msg} vs {}", c.code());
        }
    }

    #[test]
    fn every_code_is_unique() {
        let codes = [
            WorldError::BadMagic.code(),
            WorldError::BadKind(0).code(),
            WorldError::Varint(0).code(),
            WorldError::BadString(0).code(),
            WorldError::Trailing(0).code(),
            WorldError::UnknownEnt(0).code(),
            WorldError::UnknownObject(0).code(),
            WorldError::DuplicateEnt(0).code(),
        ];
        let mut sorted = codes.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len());
    }
}
