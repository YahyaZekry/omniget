//! Interest management: the client is only told about what is near it.
//!
//! The same idea the TV uses (`set_subscribed` by distance, plan §2.1 line 20)
//! applied to state instead of video. Offline it keeps the diff small; online
//! it is also what stops a visitor from reading the whole house out of the
//! wire.

use crate::map::Tile;

/// A circle of attention. `radius` is in tiles; `u16` because a house is not
/// 65 000 tiles wide and the field costs one varint.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Interest {
    pub center: Tile,
    pub radius: u16,
}

impl Interest {
    /// Everything, for the local player who is looking at the whole house.
    pub const ALL: Interest = Interest {
        center: Tile { x: 0, y: 0 },
        radius: u16::MAX,
    };

    pub const fn new(center: Tile, radius: u16) -> Interest {
        Interest { center, radius }
    }

    /// Nothing is filtered when the radius is the maximum, which saves the
    /// distance test on the common local path.
    pub const fn is_everything(&self) -> bool {
        self.radius == u16::MAX
    }

    /// Is this tile inside the circle. Squared distance, no square root.
    pub fn covers(&self, t: Tile) -> bool {
        if self.is_everything() {
            return true;
        }
        let r = self.radius as u64;
        t.dist2(self.center) <= r * r
    }

    /// A slightly wider circle, used to decide what to *keep* sending once it
    /// is already on screen: an agent walking on the boundary would otherwise
    /// flicker in and out of the diff every other tick.
    pub fn hysteresis(&self) -> Interest {
        if self.is_everything() {
            return *self;
        }
        Interest {
            center: self.center,
            radius: self.radius.saturating_add(self.radius / 8 + 1),
        }
    }
}

impl Default for Interest {
    fn default() -> Interest {
        Interest::ALL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_covers_everything() {
        let i = Interest::ALL;
        assert!(i.is_everything());
        assert!(i.covers(Tile::new(100_000, -100_000)));
        assert_eq!(Interest::default(), Interest::ALL);
    }

    #[test]
    fn a_circle_is_a_circle_not_a_square() {
        let i = Interest::new(Tile::new(0, 0), 10);
        assert!(i.covers(Tile::new(10, 0)));
        assert!(i.covers(Tile::new(7, 7)));
        assert!(!i.covers(Tile::new(8, 8)), "sqrt(128) > 10");
        assert!(!i.covers(Tile::new(11, 0)));
    }

    #[test]
    fn radius_zero_is_the_centre_tile_only() {
        let i = Interest::new(Tile::new(3, 4), 0);
        assert!(i.covers(Tile::new(3, 4)));
        assert!(!i.covers(Tile::new(3, 5)));
    }

    #[test]
    fn hysteresis_is_wider_and_never_overflows() {
        let i = Interest::new(Tile::new(0, 0), 16);
        let h = i.hysteresis();
        assert!(h.radius > i.radius);
        assert!(h.covers(Tile::new(17, 0)));
        assert_eq!(Interest::ALL.hysteresis(), Interest::ALL);
        assert_eq!(
            Interest::new(Tile::new(0, 0), u16::MAX - 1)
                .hysteresis()
                .radius,
            u16::MAX
        );
    }
}
