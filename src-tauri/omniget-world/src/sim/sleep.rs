//! Sleep states: what the world costs when nobody is looking at it.
//!
//! The budget (plan §1.2) says an app that never opens `/world` spends zero,
//! and a world whose route is closed spends at most half a millisecond of CPU
//! per second. That is not something a renderer can decide; it is a state of
//! the simulation, and it is the reason `catch_up` exists: eight hours of a
//! closed laptop must not become 288 000 ticks of replay.

/// Milliseconds of one simulation tick. 10 Hz, fixed, forever: the number is
/// part of the determinism contract.
pub const TICK_MS: u64 = 100;

/// How often the tick runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SleepState {
    /// The route is visible: 10 Hz.
    #[default]
    Active,
    /// The route is closed but the app is open: 0.2 Hz, and the ticks in
    /// between are made up by `catch_up`.
    Dozing,
    /// The app is minimised and has been for a while: nothing runs at all
    /// until something wakes the world.
    Hibernating,
}

impl SleepState {
    /// Milliseconds between calls to `step()`, or `None` when nothing should
    /// run.
    pub const fn interval_ms(self) -> Option<u64> {
        match self {
            SleepState::Active => Some(TICK_MS),
            SleepState::Dozing => Some(5_000),
            SleepState::Hibernating => None,
        }
    }

    /// Simulation ticks one `step()` stands for in this state. Dozing runs one
    /// real tick and catches the other 49 up in bulk.
    pub const fn stride(self) -> u64 {
        match self {
            SleepState::Active => 1,
            SleepState::Dozing => 50,
            SleepState::Hibernating => 0,
        }
    }

    pub const fn tag(self) -> u8 {
        match self {
            SleepState::Active => 0,
            SleepState::Dozing => 1,
            SleepState::Hibernating => 2,
        }
    }

    pub const fn from_tag(tag: u8) -> Option<SleepState> {
        Some(match tag {
            0 => SleepState::Active,
            1 => SleepState::Dozing,
            2 => SleepState::Hibernating,
            _ => return None,
        })
    }
}

/// Whole ticks in a span of real time, truncating.
#[inline]
pub const fn ticks_from_ms(ms: u64) -> u64 {
    ms / TICK_MS
}

/// Ticks that a catch-up may cover in one call: 24 hours of wall clock. More
/// than that and the world is better off restarting its day than pretending it
/// lived through it.
pub const MAX_CATCH_UP_TICKS: u64 = 24 * 60 * 60 * 1000 / TICK_MS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_match_the_budget() {
        assert_eq!(SleepState::Active.interval_ms(), Some(100));
        assert_eq!(SleepState::Dozing.interval_ms(), Some(5_000));
        assert_eq!(SleepState::Hibernating.interval_ms(), None);
    }

    #[test]
    fn dozing_covers_the_gap_it_leaves() {
        let s = SleepState::Dozing;
        let gap = s.interval_ms().unwrap();
        assert_eq!(ticks_from_ms(gap), s.stride());
    }

    #[test]
    fn tags_round_trip() {
        for s in [
            SleepState::Active,
            SleepState::Dozing,
            SleepState::Hibernating,
        ] {
            assert_eq!(SleepState::from_tag(s.tag()), Some(s));
        }
        assert_eq!(SleepState::from_tag(7), None);
    }

    #[test]
    fn eight_hours_is_within_the_catch_up_limit() {
        assert_eq!(ticks_from_ms(8 * 3_600_000), 288_000);
        assert!(ticks_from_ms(8 * 3_600_000) < MAX_CATCH_UP_TICKS);
        assert_eq!(MAX_CATCH_UP_TICKS, 864_000);
    }

    #[test]
    fn sub_tick_spans_are_zero_ticks() {
        assert_eq!(ticks_from_ms(0), 0);
        assert_eq!(ticks_from_ms(99), 0);
        assert_eq!(ticks_from_ms(100), 1);
        assert_eq!(ticks_from_ms(150), 1);
    }
}
