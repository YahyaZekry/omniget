//! Height. The renderer sorts by `y + z` (see `src/lib/world/render/types.ts`,
//! `TILE_Z = 32`), so one z unit is 32 px of screen height and the atlas's
//! `height` field — 0 for floors, 32 for casa-v1 walls — converts straight
//! into it. Storing height per tile from day one is the whole reason a
//! character walks behind a wall instead of through it.

use crate::fixed::{Fixed, ONE};

/// Screen pixels of one world z unit. Must match `TILE_Z` in the renderer.
pub const PX_PER_Z: i32 = 32;

/// Atlas pixel height -> world z in fixed point.
#[inline]
pub fn px_to_z(px: u16) -> Fixed {
    Fixed((px as i32 * ONE) / PX_PER_Z)
}

/// World z -> atlas pixels, truncating. Only used for round-trip checks.
#[inline]
pub fn z_to_px(z: Fixed) -> u16 {
    ((z.0 * PX_PER_Z) / ONE).clamp(0, u16::MAX as i32) as u16
}

/// Height field over the map rectangle, one byte of atlas pixels per tile.
/// Kept apart from the tile palette because the sim reads it every tick and
/// the palette only on load.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HeightField {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl HeightField {
    pub fn new(w: usize, h: usize) -> HeightField {
        HeightField {
            w,
            h,
            px: vec![0; w * h],
        }
    }

    pub const fn width(&self) -> usize {
        self.w
    }

    pub const fn height(&self) -> usize {
        self.h
    }

    #[inline]
    pub fn get_px(&self, x: usize, y: usize) -> u8 {
        if x >= self.w || y >= self.h {
            return 0;
        }
        self.px[y * self.w + x]
    }

    #[inline]
    pub fn set_px(&mut self, x: usize, y: usize, v: u8) {
        if x < self.w && y < self.h {
            self.px[y * self.w + x] = v;
        }
    }

    /// Standing height of a tile in world z.
    #[inline]
    pub fn z_at(&self, x: usize, y: usize) -> Fixed {
        px_to_z(self.get_px(x, y) as u16)
    }

    /// A step of more than one z unit is a climb, and agents do not climb.
    #[inline]
    pub fn step_is_walkable(&self, from: (usize, usize), to: (usize, usize)) -> bool {
        let a = self.get_px(from.0, from.1) as i32;
        let b = self.get_px(to.0, to.1) as i32;
        (a - b).abs() <= PX_PER_Z
    }

    pub fn raw(&self) -> &[u8] {
        &self.px
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_height_is_one_z_unit() {
        assert_eq!(px_to_z(32), Fixed::ONE);
        assert_eq!(px_to_z(0), Fixed::ZERO);
        assert_eq!(px_to_z(16), Fixed::HALF);
    }

    #[test]
    fn z_round_trips_through_pixels() {
        for px in [0u16, 14, 16, 20, 28, 32, 64] {
            assert_eq!(z_to_px(px_to_z(px)), px);
        }
    }

    #[test]
    fn out_of_range_reads_are_flat_not_panics() {
        let f = HeightField::new(4, 4);
        assert_eq!(f.get_px(99, 99), 0);
        assert_eq!(f.z_at(99, 0), Fixed::ZERO);
    }

    #[test]
    fn a_one_unit_step_is_walkable_but_two_are_not() {
        let mut f = HeightField::new(3, 1);
        f.set_px(1, 0, 32);
        f.set_px(2, 0, 96);
        assert!(f.step_is_walkable((0, 0), (1, 0)));
        assert!(!f.step_is_walkable((1, 0), (2, 0)));
    }
}
