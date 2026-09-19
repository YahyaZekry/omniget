//! A* on the tile grid, with every buffer reused between calls.
//!
//! The open set is a binary heap kept in a plain `Vec` rather than
//! `BinaryHeap`, so ties break on the cell index and two machines expand the
//! same nodes in the same order. Visited marks are generation stamps instead
//! of a cleared array, so a search over a 64x64 map costs no allocation and no
//! `memset` once the world has warmed up.

use crate::map::Tile;
use crate::path::grid::{Grid, DIAG, STRAIGHT};

/// Eight neighbours, in a fixed order. Diagonals last so that, at equal cost,
/// the straight move is found first and paths look deliberate.
const NEIGHBOURS: [(i32, i32, u32); 8] = [
    (0, -1, STRAIGHT),
    (1, 0, STRAIGHT),
    (0, 1, STRAIGHT),
    (-1, 0, STRAIGHT),
    (1, -1, DIAG),
    (1, 1, DIAG),
    (-1, 1, DIAG),
    (-1, -1, DIAG),
];

/// Hard ceiling on expanded nodes, so a hopeless search cannot stall a tick.
pub const MAX_EXPANSIONS: u32 = 16_384;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Node {
    f: u32,
    idx: u32,
}

impl Node {
    /// Lower f wins; equal f breaks on the cell index, never on insertion
    /// order, so the result does not depend on the allocator.
    #[inline]
    fn better(self, other: Node) -> bool {
        (self.f, self.idx) < (other.f, other.idx)
    }
}

/// Reusable A* workspace. One per world; the tick borrows it mutably.
#[derive(Clone, Debug, Default)]
pub struct AStar {
    open: Vec<Node>,
    g: Vec<u32>,
    came: Vec<u32>,
    stamp: Vec<u32>,
    gen: u32,
    /// Nodes expanded by the last search, for the bench.
    pub last_expansions: u32,
}

impl AStar {
    pub fn new() -> AStar {
        AStar::default()
    }

    fn ensure(&mut self, n: usize) {
        if self.g.len() != n {
            self.g = vec![0; n];
            self.came = vec![u32::MAX; n];
            self.stamp = vec![0; n];
            self.gen = 0;
        }
    }

    /// Route from `from` to `to`, written into `out` as the tiles to visit,
    /// excluding the starting tile. Returns false and leaves `out` empty when
    /// there is no route.
    pub fn find(&mut self, grid: &Grid, from: Tile, to: Tile, out: &mut Vec<Tile>) -> bool {
        out.clear();
        self.last_expansions = 0;
        self.ensure(grid.len());

        let (Some(start), Some(goal)) = (grid.index(from), grid.index(to)) else {
            return false;
        };
        if !grid.passable_at(goal) {
            return false;
        }
        if start == goal {
            return true;
        }

        self.gen = self.gen.wrapping_add(1);
        if self.gen == 0 {
            // Wrapped after 4 billion searches: clear once rather than risk a
            // stale stamp matching.
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.gen = 1;
        }
        let gen = self.gen;
        self.open.clear();

        self.g[start] = 0;
        self.came[start] = u32::MAX;
        self.stamp[start] = gen;
        let h0 = heuristic(grid, start, goal);
        push(
            &mut self.open,
            Node {
                f: h0,
                idx: start as u32,
            },
        );

        while let Some(node) = pop(&mut self.open) {
            let cur = node.idx as usize;
            if cur == goal {
                self.reconstruct(grid, start, goal, out);
                return true;
            }
            self.last_expansions += 1;
            if self.last_expansions > MAX_EXPANSIONS {
                return false;
            }
            let cur_tile = grid.tile_of(cur);
            let g_cur = self.g[cur];
            for (dx, dy, step) in NEIGHBOURS {
                let nt = Tile::new(cur_tile.x + dx, cur_tile.y + dy);
                let Some(ni) = grid.index(nt) else { continue };
                if !grid.passable_at(ni) || !grid.step_ok(cur, ni) {
                    continue;
                }
                if dx != 0 && dy != 0 {
                    // No cutting a corner between two blocked tiles, and no
                    // squeezing diagonally past a single one either: the
                    // sprite would clip through the wall it walks beside.
                    let side_a = grid.index(Tile::new(cur_tile.x + dx, cur_tile.y));
                    let side_b = grid.index(Tile::new(cur_tile.x, cur_tile.y + dy));
                    let ok = side_a.map(|i| grid.passable_at(i)).unwrap_or(false)
                        && side_b.map(|i| grid.passable_at(i)).unwrap_or(false);
                    if !ok {
                        continue;
                    }
                }
                let tentative = g_cur + step;
                let seen = self.stamp[ni] == gen;
                if seen && self.g[ni] <= tentative {
                    continue;
                }
                self.stamp[ni] = gen;
                self.g[ni] = tentative;
                self.came[ni] = cur as u32;
                push(
                    &mut self.open,
                    Node {
                        f: tentative + heuristic(grid, ni, goal),
                        idx: ni as u32,
                    },
                );
            }
        }
        false
    }

    fn reconstruct(&self, grid: &Grid, start: usize, goal: usize, out: &mut Vec<Tile>) {
        let mut cur = goal;
        while cur != start {
            out.push(grid.tile_of(cur));
            let prev = self.came[cur];
            if prev == u32::MAX {
                break;
            }
            cur = prev as usize;
        }
        out.reverse();
    }
}

/// Octile distance: exact on an empty grid, never an over-estimate on a full
/// one, which is what keeps A* optimal.
#[inline]
fn heuristic(grid: &Grid, a: usize, b: usize) -> u32 {
    let (ax, ay) = ((a % grid.w) as i32, (a / grid.w) as i32);
    let (bx, by) = ((b % grid.w) as i32, (b / grid.w) as i32);
    let dx = (ax - bx).unsigned_abs();
    let dy = (ay - by).unsigned_abs();
    let (lo, hi) = if dx < dy { (dx, dy) } else { (dy, dx) };
    STRAIGHT * hi + (DIAG - STRAIGHT) * lo
}

fn push(heap: &mut Vec<Node>, n: Node) {
    heap.push(n);
    let mut i = heap.len() - 1;
    while i > 0 {
        let parent = (i - 1) / 2;
        if heap[i].better(heap[parent]) {
            heap.swap(i, parent);
            i = parent;
        } else {
            break;
        }
    }
}

fn pop(heap: &mut Vec<Node>) -> Option<Node> {
    if heap.is_empty() {
        return None;
    }
    let top = heap[0];
    let last = heap.pop()?;
    if heap.is_empty() {
        return Some(top);
    }
    heap[0] = last;
    let mut i = 0;
    loop {
        let (l, r) = (2 * i + 1, 2 * i + 2);
        let mut best = i;
        if l < heap.len() && heap[l].better(heap[best]) {
            best = l;
        }
        if r < heap.len() && heap[r].better(heap[best]) {
            best = r;
        }
        if best == i {
            break;
        }
        heap.swap(i, best);
        i = best;
    }
    Some(top)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::fixtures::one_room;
    use crate::map::Map;

    fn grid() -> Grid {
        Grid::from_map(&Map::load(&one_room()).unwrap())
    }

    #[test]
    fn straight_line_on_an_open_floor() {
        let g = grid();
        let mut a = AStar::new();
        let mut out = Vec::new();
        assert!(a.find(&g, Tile::new(2, 5), Tile::new(6, 5), &mut out));
        assert_eq!(out.len(), 4);
        assert_eq!(out.last(), Some(&Tile::new(6, 5)));
        assert_eq!(out[0], Tile::new(3, 5));
    }

    #[test]
    fn diagonal_costs_less_than_two_straights() {
        let g = grid();
        let mut a = AStar::new();
        let mut out = Vec::new();
        assert!(a.find(&g, Tile::new(2, 2), Tile::new(6, 6), &mut out));
        assert_eq!(out.len(), 4, "four diagonals, not eight straights");
    }

    #[test]
    fn same_tile_needs_no_steps() {
        let g = grid();
        let mut a = AStar::new();
        let mut out = vec![Tile::new(9, 9)];
        assert!(a.find(&g, Tile::new(2, 2), Tile::new(2, 2), &mut out));
        assert!(out.is_empty());
    }

    #[test]
    fn a_blocked_goal_has_no_route() {
        let g = grid();
        let mut a = AStar::new();
        let mut out = Vec::new();
        assert!(!a.find(&g, Tile::new(2, 2), Tile::new(2, 0), &mut out));
        assert!(!a.find(&g, Tile::new(2, 2), Tile::new(99, 99), &mut out));
        assert!(!a.find(&g, Tile::new(99, 99), Tile::new(2, 2), &mut out));
        assert!(out.is_empty());
    }

    #[test]
    fn a_wall_across_the_room_is_walked_around() {
        let mut g = grid();
        for y in 1..15 {
            g.block(Tile::new(8, y));
        }
        let mut a = AStar::new();
        let mut out = Vec::new();
        assert!(a.find(&g, Tile::new(2, 5), Tile::new(12, 5), &mut out));
        assert!(out.iter().all(|t| !(t.x == 8 && (1..15).contains(&t.y))));
        assert_eq!(out.last(), Some(&Tile::new(12, 5)));
    }

    #[test]
    fn a_sealed_room_has_no_route() {
        let mut g = grid();
        for y in 1..16 {
            g.block(Tile::new(8, y));
        }
        let mut a = AStar::new();
        let mut out = Vec::new();
        assert!(!a.find(&g, Tile::new(2, 5), Tile::new(12, 5), &mut out));
    }

    #[test]
    fn corners_are_not_cut() {
        let mut g = grid();
        g.block(Tile::new(5, 4));
        g.block(Tile::new(4, 5));
        let mut a = AStar::new();
        let mut out = Vec::new();
        assert!(a.find(&g, Tile::new(4, 4), Tile::new(5, 5), &mut out));
        assert!(out.len() > 1, "cannot slip diagonally between two blocks");
    }

    #[test]
    fn the_same_query_gives_the_same_path_every_time() {
        let g = grid();
        let mut a = AStar::new();
        let mut first = Vec::new();
        a.find(&g, Tile::new(1, 1), Tile::new(14, 14), &mut first);
        for _ in 0..20 {
            let mut again = Vec::new();
            a.find(&g, Tile::new(1, 1), Tile::new(14, 14), &mut again);
            assert_eq!(first, again);
        }
        // And a fresh workspace agrees with a reused one.
        let mut fresh = AStar::new();
        let mut out = Vec::new();
        fresh.find(&g, Tile::new(1, 1), Tile::new(14, 14), &mut out);
        assert_eq!(first, out);
    }

    #[test]
    fn heap_pops_in_order() {
        let mut h: Vec<Node> = Vec::new();
        for f in [5u32, 1, 9, 3, 3, 7] {
            push(&mut h, Node { f, idx: f });
        }
        let mut got = Vec::new();
        while let Some(n) = pop(&mut h) {
            got.push(n.f);
        }
        assert_eq!(got, vec![1, 3, 3, 5, 7, 9]);
    }
}
