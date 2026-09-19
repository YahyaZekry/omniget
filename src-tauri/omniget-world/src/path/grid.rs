//! The walkability grid the pathfinder reads.
//!
//! Static walkability comes from the map's chunk layers and never changes:
//! the house has fixed walls by design (plan §9.1). Objects are the only
//! dynamic part, and they are counted rather than flagged, so two objects
//! sharing a tile do not unblock it when one of them is taken away.

use crate::map::{Map, Tile};

/// Grid cost of one orthogonal step. Diagonals cost `DIAG` so that a diagonal
/// is worth about sqrt(2) straights without any floating point.
pub const STRAIGHT: u32 = 10;
pub const DIAG: u32 = 14;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Grid {
    pub origin: Tile,
    pub w: usize,
    pub h: usize,
    walk: Vec<bool>,
    blocked: Vec<u16>,
    height: Vec<u8>,
}

impl Grid {
    pub fn from_map(map: &Map) -> Grid {
        Grid {
            origin: map.origin,
            w: map.w,
            h: map.h,
            walk: map.static_walk().to_vec(),
            blocked: vec![0; map.w * map.h],
            height: map.heights().raw().to_vec(),
        }
    }

    #[inline]
    pub fn index(&self, t: Tile) -> Option<usize> {
        let x = t.x - self.origin.x;
        let y = t.y - self.origin.y;
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return None;
        }
        Some(y as usize * self.w + x as usize)
    }

    #[inline]
    pub fn tile_of(&self, i: usize) -> Tile {
        Tile::new(
            self.origin.x + (i % self.w) as i32,
            self.origin.y + (i / self.w) as i32,
        )
    }

    /// Can an agent stand here right now.
    #[inline]
    pub fn passable(&self, t: Tile) -> bool {
        match self.index(t) {
            Some(i) => self.walk[i] && self.blocked[i] == 0,
            None => false,
        }
    }

    #[inline]
    pub fn passable_at(&self, i: usize) -> bool {
        self.walk[i] && self.blocked[i] == 0
    }

    /// A step of more than one z unit is a climb; agents walk, they do not
    /// climb.
    #[inline]
    pub fn step_ok(&self, from: usize, to: usize) -> bool {
        (self.height[from] as i32 - self.height[to] as i32).abs() <= 32
    }

    pub fn block(&mut self, t: Tile) {
        if let Some(i) = self.index(t) {
            self.blocked[i] = self.blocked[i].saturating_add(1);
        }
    }

    pub fn unblock(&mut self, t: Tile) {
        if let Some(i) = self.index(t) {
            self.blocked[i] = self.blocked[i].saturating_sub(1);
        }
    }

    /// Block or unblock the whole footprint of an object.
    pub fn set_footprint(&mut self, at: Tile, footprint: [u8; 2], blocked: bool) {
        let w = footprint[0].max(1) as i32;
        let d = footprint[1].max(1) as i32;
        for dy in 0..d {
            for dx in 0..w {
                let t = Tile::new(at.x + dx, at.y + dy);
                if blocked {
                    self.block(t);
                } else {
                    self.unblock(t);
                }
            }
        }
    }

    /// Nearest tile an agent can actually stand on, objects included, searched
    /// in rings so the answer does not depend on iteration order. This is the
    /// one to use before spawning or routing: `Map::nearest_walkable` only
    /// knows about walls, and a bed is not a wall.
    pub fn nearest_passable(&self, t: Tile, max_ring: i32) -> Option<Tile> {
        if self.passable(t) {
            return Some(t);
        }
        for r in 1..=max_ring {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r {
                        continue;
                    }
                    let c = Tile::new(t.x + dx, t.y + dy);
                    if self.passable(c) {
                        return Some(c);
                    }
                }
            }
        }
        None
    }

    pub fn len(&self) -> usize {
        self.walk.len()
    }

    pub fn is_empty(&self) -> bool {
        self.walk.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::fixtures::one_room;

    #[test]
    fn static_walkability_comes_from_the_map() {
        let map = Map::load(&one_room()).unwrap();
        let g = Grid::from_map(&map);
        assert!(g.passable(Tile::new(3, 3)));
        assert!(!g.passable(Tile::new(3, 0)));
        assert!(!g.passable(Tile::new(-1, 3)));
        assert_eq!(g.len(), 256);
    }

    #[test]
    fn blocking_counts_instead_of_flagging() {
        let map = Map::load(&one_room()).unwrap();
        let mut g = Grid::from_map(&map);
        let t = Tile::new(5, 5);
        g.block(t);
        g.block(t);
        assert!(!g.passable(t));
        g.unblock(t);
        assert!(!g.passable(t), "still blocked by the second object");
        g.unblock(t);
        assert!(g.passable(t));
        g.unblock(t);
        assert!(g.passable(t), "unblocking below zero must not wrap");
    }

    #[test]
    fn footprints_block_every_tile_they_cover() {
        let map = Map::load(&one_room()).unwrap();
        let mut g = Grid::from_map(&map);
        g.set_footprint(Tile::new(4, 4), [2, 1], true);
        assert!(!g.passable(Tile::new(4, 4)));
        assert!(!g.passable(Tile::new(5, 4)));
        assert!(g.passable(Tile::new(6, 4)));
        g.set_footprint(Tile::new(4, 4), [2, 1], false);
        assert!(g.passable(Tile::new(4, 4)));
    }

    #[test]
    fn nearest_passable_steps_around_furniture() {
        let map = Map::load(&one_room()).unwrap();
        let mut g = Grid::from_map(&map);
        g.set_footprint(Tile::new(4, 4), [2, 2], true);
        assert_eq!(
            g.nearest_passable(Tile::new(9, 9), 4),
            Some(Tile::new(9, 9))
        );
        let out = g.nearest_passable(Tile::new(4, 4), 4).unwrap();
        assert!(g.passable(out));
        assert!(out.chebyshev(Tile::new(4, 4)) <= 2);
        assert!(g.nearest_passable(Tile::new(500, 500), 3).is_none());
    }

    #[test]
    fn index_and_tile_of_are_inverses() {
        let map = Map::load(&one_room()).unwrap();
        let g = Grid::from_map(&map);
        for t in [Tile::new(0, 0), Tile::new(15, 15), Tile::new(7, 2)] {
            assert_eq!(g.tile_of(g.index(t).unwrap()), t);
        }
        assert!(g.index(Tile::new(16, 0)).is_none());
    }

    #[test]
    fn a_full_z_step_is_allowed_and_more_is_not() {
        let mut def = one_room();
        def.chunks[0].height = vec![0; 256];
        def.chunks[0].height[crate::map::Chunk::index(5, 5)] = 32;
        def.chunks[0].height[crate::map::Chunk::index(6, 5)] = 96;
        let map = Map::load(&def).unwrap();
        let g = Grid::from_map(&map);
        let a = g.index(Tile::new(4, 5)).unwrap();
        let b = g.index(Tile::new(5, 5)).unwrap();
        let c = g.index(Tile::new(6, 5)).unwrap();
        assert!(g.step_ok(a, b));
        assert!(!g.step_ok(b, c));
    }
}
