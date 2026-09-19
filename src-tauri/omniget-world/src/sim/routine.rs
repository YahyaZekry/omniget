//! Daily routines: what an agent does when nobody told it anything.
//!
//! A routine is a sorted list of "at this minute of the game day, decide
//! this". It costs no model call, which is the point: the house is alive
//! whether or not a single token is spent (plan §9.1, "nada disso chama
//! modelo").

use crate::sim::mailbox::Decision;

/// Ticks in one game minute. At 10 Hz this makes a game day 19.2 real
/// minutes, close enough to the genre that a session sees a whole day.
pub const TICKS_PER_GAME_MINUTE: u64 = 8;
/// Minutes in a game day.
pub const MINUTES_PER_DAY: u64 = 24 * 60;
/// Ticks in a game day.
pub const TICKS_PER_DAY: u64 = TICKS_PER_GAME_MINUTE * MINUTES_PER_DAY;

/// Game minute a tick falls in, 0..1440.
#[inline]
pub const fn minute_of_day(tick: u64) -> u16 {
    ((tick / TICKS_PER_GAME_MINUTE) % MINUTES_PER_DAY) as u16
}

/// Game day a tick falls in, counted from world start.
#[inline]
pub const fn day_of(tick: u64) -> u64 {
    tick / TICKS_PER_DAY
}

/// `13 * 60 + 30` reads worse than `hm(13, 30)` in a table of a dozen entries.
#[inline]
pub const fn hm(h: u16, m: u16) -> u16 {
    (h * 60 + m) % MINUTES_PER_DAY as u16
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoutineEntry {
    /// Minute of the game day, 0..1440.
    pub minute: u16,
    pub decision: Decision,
}

/// An agent's day. Entries are kept sorted by minute; two entries in the same
/// minute fire in the order they were added.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Routine {
    entries: Vec<RoutineEntry>,
}

impl Routine {
    pub fn new(entries: Vec<RoutineEntry>) -> Routine {
        let mut r = Routine { entries };
        r.entries.sort_by_key(|e| e.minute);
        r
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> &[RoutineEntry] {
        &self.entries
    }

    pub fn push(&mut self, minute: u16, decision: Decision) {
        let e = RoutineEntry { minute, decision };
        let pos = self.entries.partition_point(|x| x.minute <= e.minute);
        self.entries.insert(pos, e);
    }

    /// Entries due exactly at this minute.
    pub fn due(&self, minute: u16) -> impl Iterator<Item = &Decision> {
        let start = self.entries.partition_point(|e| e.minute < minute);
        self.entries[start..]
            .iter()
            .take_while(move |e| e.minute == minute)
            .map(|e| &e.decision)
    }

    /// What the routine says the agent should be doing at this minute: the
    /// last entry at or before it, wrapping past midnight. This is what
    /// `catch_up` uses to place an agent after hours of not simulating,
    /// instead of replaying every tick.
    pub fn current(&self, minute: u16) -> Option<&Decision> {
        if self.entries.is_empty() {
            return None;
        }
        let pos = self.entries.partition_point(|e| e.minute <= minute);
        if pos == 0 {
            // Before the first entry of the day: yesterday's last one still
            // holds.
            self.entries.last().map(|e| &e.decision)
        } else {
            self.entries.get(pos - 1).map(|e| &e.decision)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ents::id::ObjectId;
    use crate::map::Tile;

    fn sample() -> Routine {
        Routine::new(vec![
            RoutineEntry {
                minute: hm(22, 0),
                decision: Decision::Sleep,
            },
            RoutineEntry {
                minute: hm(8, 0),
                decision: Decision::Work(ObjectId(1)),
            },
            RoutineEntry {
                minute: hm(12, 0),
                decision: Decision::GoTo(Tile::new(3, 3)),
            },
        ])
    }

    #[test]
    fn clock_helpers() {
        assert_eq!(hm(0, 0), 0);
        assert_eq!(hm(13, 30), 810);
        assert_eq!(minute_of_day(0), 0);
        assert_eq!(minute_of_day(TICKS_PER_GAME_MINUTE), 1);
        assert_eq!(minute_of_day(TICKS_PER_DAY), 0);
        assert_eq!(day_of(TICKS_PER_DAY - 1), 0);
        assert_eq!(day_of(TICKS_PER_DAY), 1);
    }

    #[test]
    fn entries_are_sorted_whatever_the_order_in() {
        let r = sample();
        let mins: Vec<u16> = r.entries().iter().map(|e| e.minute).collect();
        assert_eq!(mins, vec![hm(8, 0), hm(12, 0), hm(22, 0)]);
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn due_fires_only_on_the_exact_minute() {
        let r = sample();
        assert_eq!(r.due(hm(8, 0)).count(), 1);
        assert_eq!(r.due(hm(8, 1)).count(), 0);
        assert_eq!(
            r.due(hm(12, 0)).next(),
            Some(&Decision::GoTo(Tile::new(3, 3)))
        );
    }

    #[test]
    fn two_entries_in_one_minute_both_fire() {
        let mut r = Routine::default();
        r.push(hm(9, 0), Decision::Idle);
        r.push(hm(9, 0), Decision::Sleep);
        let got: Vec<u8> = r.due(hm(9, 0)).map(|d| d.tag()).collect();
        assert_eq!(got, vec![Decision::Idle.tag(), Decision::Sleep.tag()]);
    }

    #[test]
    fn current_wraps_past_midnight() {
        let r = sample();
        assert_eq!(r.current(hm(9, 0)), Some(&Decision::Work(ObjectId(1))));
        assert_eq!(r.current(hm(23, 0)), Some(&Decision::Sleep));
        assert_eq!(
            r.current(hm(3, 0)),
            Some(&Decision::Sleep),
            "3am is still last night's entry"
        );
        assert_eq!(Routine::default().current(0), None);
    }

    #[test]
    fn a_day_is_the_advertised_length() {
        assert_eq!(TICKS_PER_DAY, 11_520);
        // 11520 ticks at 10 Hz = 1152 s = 19.2 real minutes.
        assert_eq!(TICKS_PER_DAY / 10, 1152);
    }
}
