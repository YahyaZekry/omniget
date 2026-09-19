//! What an agent is doing and how tired it is.

use crate::ents::id::{EntId, ObjectId};

/// Facing, clockwise from south, matching the eight atlas directions. SE, E
/// and NE are drawn by mirroring SW, W and NW, which is the renderer's
/// business, not the simulation's.
pub const DIR_S: u8 = 0;
pub const DIR_SW: u8 = 1;
pub const DIR_W: u8 = 2;
pub const DIR_NW: u8 = 3;
pub const DIR_N: u8 = 4;
pub const DIR_NE: u8 = 5;
pub const DIR_E: u8 = 6;
pub const DIR_SE: u8 = 7;

/// Animation clip ids, the `anim` byte on the wire. The renderer maps these to
/// `<sheet>/<anim>` atlas keys; numbers travel, strings do not.
pub const ANIM_IDLE: u8 = 0;
pub const ANIM_WALK: u8 = 1;
pub const ANIM_SIT: u8 = 2;
pub const ANIM_SLEEP: u8 = 3;
pub const ANIM_WORK: u8 = 4;
pub const ANIM_TALK: u8 = 5;
pub const ANIM_WAVE: u8 = 6;
pub const ANIM_YAWN: u8 = 7;
/// One past the last clip, so a decoder can reject nonsense.
pub const ANIM_COUNT: u8 = 8;

/// Energy is the account's remaining quota, pushed in by `Input::SetEnergy`
/// (plan §9.1). The simulation never asks a model for it and never invents it.
pub const ENERGY_FULL: u8 = 255;
/// Below this the agent yawns and walks slower.
pub const ENERGY_TIRED: u8 = 64;
/// Below this the agent goes to bed on its own.
pub const ENERGY_SPENT: u8 = 16;

/// What an agent is doing right now. The tag is what goes on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Activity {
    #[default]
    Idle,
    Walking,
    Sitting(ObjectId),
    Sleeping,
    Working(ObjectId),
    Talking,
    Waving(EntId),
    Yawning,
}

impl Activity {
    pub const fn tag(self) -> u8 {
        match self {
            Activity::Idle => 0,
            Activity::Walking => 1,
            Activity::Sitting(_) => 2,
            Activity::Sleeping => 3,
            Activity::Working(_) => 4,
            Activity::Talking => 5,
            Activity::Waving(_) => 6,
            Activity::Yawning => 7,
        }
    }

    /// The id the tag carries, or zero.
    pub const fn arg(self) -> u32 {
        match self {
            Activity::Sitting(o) | Activity::Working(o) => o.0,
            Activity::Waving(e) => e.0,
            _ => 0,
        }
    }

    pub fn from_parts(tag: u8, arg: u32) -> Option<Activity> {
        Some(match tag {
            0 => Activity::Idle,
            1 => Activity::Walking,
            2 => Activity::Sitting(ObjectId(arg)),
            3 => Activity::Sleeping,
            4 => Activity::Working(ObjectId(arg)),
            5 => Activity::Talking,
            6 => Activity::Waving(EntId(arg)),
            7 => Activity::Yawning,
            _ => return None,
        })
    }

    /// The clip that plays while this activity lasts.
    pub const fn anim(self) -> u8 {
        match self {
            Activity::Idle => ANIM_IDLE,
            Activity::Walking => ANIM_WALK,
            Activity::Sitting(_) => ANIM_SIT,
            Activity::Sleeping => ANIM_SLEEP,
            Activity::Working(_) => ANIM_WORK,
            Activity::Talking => ANIM_TALK,
            Activity::Waving(_) => ANIM_WAVE,
            Activity::Yawning => ANIM_YAWN,
        }
    }

    /// A routine may interrupt anything but sleep; only an explicit decision
    /// or full energy wakes a sleeper.
    pub const fn interruptible_by_routine(self) -> bool {
        !matches!(self, Activity::Sleeping)
    }
}

/// Movement speed in sub-units per tick at a given energy, where the base is
/// what a rested agent does. 10 Hz and 1.5 tiles/s give 38 sub-units a tick.
pub const BASE_SPEED: i32 = 38;

/// Tired agents are slower, in whole steps so the result is the same on every
/// machine: full speed while rested, three quarters while tired, half while
/// spent.
#[inline]
pub const fn speed_for(energy: u8) -> i32 {
    if energy >= ENERGY_TIRED {
        BASE_SPEED
    } else if energy >= ENERGY_SPENT {
        BASE_SPEED * 3 / 4
    } else {
        BASE_SPEED / 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_round_trips_through_its_tag() {
        let all = [
            Activity::Idle,
            Activity::Walking,
            Activity::Sitting(ObjectId(4)),
            Activity::Sleeping,
            Activity::Working(ObjectId(9)),
            Activity::Talking,
            Activity::Waving(EntId(2)),
            Activity::Yawning,
        ];
        for a in all {
            assert_eq!(Activity::from_parts(a.tag(), a.arg()), Some(a), "{a:?}");
        }
        assert_eq!(Activity::from_parts(99, 0), None);
    }

    #[test]
    fn every_activity_has_a_clip_in_range() {
        for tag in 0..8u8 {
            let a = Activity::from_parts(tag, 1).unwrap();
            assert!(a.anim() < ANIM_COUNT);
        }
    }

    #[test]
    fn speed_falls_in_steps_with_energy() {
        assert_eq!(speed_for(ENERGY_FULL), BASE_SPEED);
        assert_eq!(speed_for(ENERGY_TIRED), BASE_SPEED);
        assert_eq!(speed_for(ENERGY_TIRED - 1), BASE_SPEED * 3 / 4);
        assert_eq!(speed_for(0), BASE_SPEED / 2);
    }

    #[test]
    fn only_sleep_resists_a_routine() {
        assert!(!Activity::Sleeping.interruptible_by_routine());
        assert!(Activity::Idle.interruptible_by_routine());
        assert!(Activity::Working(ObjectId(1)).interruptible_by_routine());
    }
}
