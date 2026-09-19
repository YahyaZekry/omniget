//! Entities: agents and placed objects.

pub mod agent;
pub mod id;
pub mod object;
pub mod soa;

pub use agent::{
    speed_for, Activity, ANIM_COUNT, ANIM_IDLE, ANIM_SIT, ANIM_SLEEP, ANIM_TALK, ANIM_WALK,
    ANIM_WAVE, ANIM_WORK, ANIM_YAWN, BASE_SPEED, DIR_E, DIR_N, DIR_NE, DIR_NW, DIR_S, DIR_SE,
    DIR_SW, DIR_W, ENERGY_FULL, ENERGY_SPENT, ENERGY_TIRED,
};
pub use id::{EntId, ObjectId};
pub use object::{kind_leaf, Object};
pub use soa::{Agents, Intent, Objects, Path};

/// Direction of travel, snapped to the eight atlas directions. Pure integer:
/// the octant is decided by comparing |dx| and |dy| against each other, never
/// by an angle.
pub fn dir_from_delta(dx: i32, dy: i32) -> u8 {
    if dx == 0 && dy == 0 {
        return DIR_S;
    }
    let ax = dx.unsigned_abs() as i64;
    let ay = dy.unsigned_abs() as i64;
    // Inside this cone the movement counts as purely along one axis. tan(22.5)
    // is about 0.4142, and 5/12 = 0.4167 is the closest small ratio.
    let narrow = ax * 12 < ay * 5;
    let wide = ay * 12 < ax * 5;
    if narrow {
        if dy > 0 {
            DIR_S
        } else {
            DIR_N
        }
    } else if wide {
        if dx > 0 {
            DIR_E
        } else {
            DIR_W
        }
    } else if dx > 0 {
        if dy > 0 {
            DIR_SE
        } else {
            DIR_NE
        }
    } else if dy > 0 {
        DIR_SW
    } else {
        DIR_NW
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cardinals_and_diagonals() {
        assert_eq!(dir_from_delta(0, 1), DIR_S);
        assert_eq!(dir_from_delta(0, -1), DIR_N);
        assert_eq!(dir_from_delta(1, 0), DIR_E);
        assert_eq!(dir_from_delta(-1, 0), DIR_W);
        assert_eq!(dir_from_delta(1, 1), DIR_SE);
        assert_eq!(dir_from_delta(-1, 1), DIR_SW);
        assert_eq!(dir_from_delta(1, -1), DIR_NE);
        assert_eq!(dir_from_delta(-1, -1), DIR_NW);
    }

    #[test]
    fn a_standing_agent_faces_south() {
        assert_eq!(dir_from_delta(0, 0), DIR_S);
    }

    #[test]
    fn shallow_angles_snap_to_the_axis() {
        assert_eq!(dir_from_delta(100, 10), DIR_E);
        assert_eq!(dir_from_delta(10, 100), DIR_S);
        assert_eq!(dir_from_delta(-100, -10), DIR_W);
    }

    #[test]
    fn every_direction_is_in_range() {
        for dx in -3..=3 {
            for dy in -3..=3 {
                assert!(dir_from_delta(dx, dy) < 8);
            }
        }
    }
}
