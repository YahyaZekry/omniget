//! Pathfinding: the walkability grid and A* over it.
//!
//! JPS is left out on purpose. It pays off on wide open grids, and the house
//! is a 16x16-chunk interior full of walls where A* already expands a few
//! hundred nodes; adding it before the bench says it is needed would be
//! guessing. `AStar::last_expansions` is there so the bench can say when that
//! changes.

pub mod astar;
pub mod grid;

pub use astar::{AStar, MAX_EXPANSIONS};
pub use grid::{Grid, DIAG, STRAIGHT};
