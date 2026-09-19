//! Entity identifiers.
//!
//! These are **world-local** dense ids, not network ids. When the house opens
//! to visitors (Fase 9), `omnidisc-proto`'s `Snowflake` stays the identity of a
//! person or an agent across machines and the room server maps it to an
//! `EntId` for the duration of a session; nothing here contradicts that, and
//! nothing here depends on it. Keeping them apart is what lets the diff spend
//! one or two bytes on an id instead of eight.

use serde::{Deserialize, Serialize};
use std::fmt;

/// An agent, a visitor or the player's own avatar.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Serialize, Deserialize)]
pub struct EntId(pub u32);

/// A placed object: furniture, the workbench, the TV.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Serialize, Deserialize)]
pub struct ObjectId(pub u32);

impl EntId {
    pub const NONE: EntId = EntId(0);
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

impl ObjectId {
    pub const NONE: ObjectId = ObjectId(0);
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Debug for EntId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "e{}", self.0)
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "o{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_the_null_id() {
        assert!(EntId::NONE.is_none());
        assert!(!EntId(1).is_none());
        assert!(ObjectId::NONE.is_none());
        assert_eq!(format!("{:?} {:?}", EntId(7), ObjectId(9)), "e7 o9");
    }

    #[test]
    fn ids_order_by_number() {
        let mut v = vec![EntId(3), EntId(1), EntId(2)];
        v.sort_unstable();
        assert_eq!(v, vec![EntId(1), EntId(2), EntId(3)]);
    }
}
