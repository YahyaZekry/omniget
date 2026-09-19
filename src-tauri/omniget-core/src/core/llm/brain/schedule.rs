//! The agenda: what an agent does when no model was called, and what energy
//! does to it. Owned by f7-world-brain.
//!
//! Everything here is a pure function of (minute of the game day, energy,
//! previous decision). No clock is read, no model is called, nothing
//! allocates per tick beyond a `Decision::Say`. That is the point of plan
//! §9.1: the house is alive whether or not a single token is spent.
//!
//! Energy is quota (plan §9.1 and §9.2): [`energy_from_capacity`] turns the
//! router's [`Capacity`] into the 0..255 the world takes through
//! `Input::SetEnergy`. Below [`LOW_ENERGY`] of the window the agent goes to
//! bed [`EARLY_BEDTIME_MINUTES`] game minutes early and yawns; at zero it
//! sleeps and the brain refuses to call a model at all until dawn.
//!
//! The tick constants mirror `omniget_world::sim::routine`. They are
//! duplicated, not imported, because `omniget-core` does not depend on
//! `omniget-world` yet; [`assert_mirror_constants`] is the test that fails if
//! the two ever drift.

use super::{Decision, ObjectId};
use crate::core::llm::router::Capacity;

/// Ticks in one game minute (`omniget_world` runs at 10 Hz).
pub const TICKS_PER_GAME_MINUTE: u64 = 8;
/// Ticks in one game hour.
pub const TICKS_PER_GAME_HOUR: u64 = TICKS_PER_GAME_MINUTE * 60;
pub const MINUTES_PER_DAY: u64 = 24 * 60;
/// Ticks in one game day: 11 520, or 19.2 real minutes.
pub const TICKS_PER_DAY: u64 = TICKS_PER_GAME_MINUTE * MINUTES_PER_DAY;

/// Full energy, as the world stores it.
pub const ENERGY_FULL: u8 = 255;
/// At or below this the agent is slower and yawns.
pub const ENERGY_TIRED: u8 = 64;
/// At or below this the agent heads for bed.
pub const ENERGY_SPENT: u8 = 16;

/// Share of the quota window under which the agent counts as low on energy.
pub const LOW_ENERGY: f32 = 0.20;
/// How much earlier a low-energy agent goes to bed, in game minutes.
pub const EARLY_BEDTIME_MINUTES: u16 = 120;

/// Default bed and wake minutes when nothing else says otherwise.
pub const DEFAULT_BEDTIME: u16 = 22 * 60;
pub const DEFAULT_WAKE: u16 = 7 * 60;

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

/// `hm(13, 30)` reads better than `13 * 60 + 30` in a table of a dozen rows.
#[inline]
pub const fn hm(h: u16, m: u16) -> u16 {
    (h * 60 + m) % MINUTES_PER_DAY as u16
}

/// `540` back to `"09:00"`, for the prompts and the log.
pub fn hhmm(minute: u16) -> String {
    let m = minute % MINUTES_PER_DAY as u16;
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Energy as a share of full, 0.0..=1.0.
#[inline]
pub fn energy_pct(energy: u8) -> f32 {
    f32::from(energy) / f32::from(ENERGY_FULL)
}

/// How tired the agent looks to the rest of the brain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Fine,
    Tired,
    Spent,
    Exhausted,
}

pub fn mood(energy: u8) -> Mood {
    match energy {
        0 => Mood::Exhausted,
        e if e <= ENERGY_SPENT => Mood::Spent,
        e if e <= ENERGY_TIRED => Mood::Tired,
        _ => Mood::Fine,
    }
}

/// Quota becomes energy (plan §9.1). The router's `Capacity` is the only
/// input: a plan window when there is one, otherwise the share of today's
/// dollar budget still unspent, otherwise full — the world never invents
/// energy and an unmetered account is never tired.
///
/// An unavailable candidate is flat zero: a logged-out account is an agent
/// asleep, not an agent that keeps trying.
pub fn energy_from_capacity(capacity: &Capacity, budget_today_usd: Option<f64>) -> u8 {
    if !capacity.available {
        return 0;
    }
    if let Some(quota) = capacity.quota_remaining {
        return share_to_energy(f64::from(quota));
    }
    match (capacity.budget_remaining_usd, budget_today_usd) {
        (Some(left), Some(total)) if total > 0.0 => share_to_energy(left / total),
        (Some(left), _) if left <= 0.0 => 0,
        _ => ENERGY_FULL,
    }
}

fn share_to_energy(share: f64) -> u8 {
    let clamped = share.clamp(0.0, 1.0);
    // Only an exactly empty window is exactly zero: 0.001 left still lets the
    // agent finish what it is doing.
    let scaled = (clamped * f64::from(ENERGY_FULL)).round() as u8;
    if clamped > 0.0 {
        scaled.max(1)
    } else {
        0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleEntry {
    /// Minute of the game day, 0..1440.
    pub minute: u16,
    pub decision: Decision,
    /// What the agent would say it is doing, for the HUD and the prompts.
    pub label: String,
}

/// An agent's day, sorted by minute. Built either by [`default_schedule`] or
/// by `plan::parse_plan` from a model answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    entries: Vec<ScheduleEntry>,
    bedtime: u16,
    wake: u16,
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule {
            entries: Vec::new(),
            bedtime: DEFAULT_BEDTIME,
            wake: DEFAULT_WAKE,
        }
    }
}

impl Schedule {
    /// Sorts the entries and derives bedtime and wake from them: bedtime is
    /// the first `Sleep`, wake is the first non-`Sleep` entry after it in
    /// cyclic order. Defaults stand in when the day has no `Sleep`.
    pub fn new(mut entries: Vec<ScheduleEntry>) -> Schedule {
        entries.sort_by_key(|e| e.minute);
        let bedtime = entries
            .iter()
            .find(|e| e.decision == Decision::Sleep)
            .map(|e| e.minute);
        let wake = match bedtime {
            Some(bed) => entries
                .iter()
                .cycle()
                .take(entries.len() * 2)
                .skip_while(|e| e.minute != bed)
                .find(|e| e.decision != Decision::Sleep)
                .map(|e| e.minute),
            None => None,
        };
        Schedule {
            entries,
            bedtime: bedtime.unwrap_or(DEFAULT_BEDTIME),
            wake: wake.unwrap_or(DEFAULT_WAKE),
        }
    }

    pub fn entries(&self) -> &[ScheduleEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn bedtime(&self) -> u16 {
        self.bedtime
    }

    pub fn wake(&self) -> u16 {
        self.wake
    }

    /// What the agenda says the agent should be doing at this minute: the last
    /// entry at or before it, wrapping past midnight.
    pub fn current(&self, minute: u16) -> Option<&ScheduleEntry> {
        if self.entries.is_empty() {
            return None;
        }
        let pos = self.entries.partition_point(|e| e.minute <= minute);
        if pos == 0 {
            self.entries.last()
        } else {
            self.entries.get(pos - 1)
        }
    }
}

/// The objects a default day needs. The ids come from `house-v1.json`
/// (f7-house-content) through the bridge; the brain never guesses them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Places {
    pub bed: ObjectId,
    pub workbench: ObjectId,
    pub table: ObjectId,
    pub sofa: ObjectId,
}

/// A day that reads like a day: wake, work, lunch, work, sofa, bed. Nine
/// entries, zero model calls, and the `Sleep`/wake pair that
/// [`hint`] shifts around when energy runs out.
pub fn default_schedule(places: &Places) -> Schedule {
    Schedule::new(vec![
        // Waking up has no destination: tile (0,0) is a wall in casa-v1.
        entry(hm(7, 0), Decision::Idle, "wake up"),
        entry(hm(7, 30), Decision::Sit(places.table), "breakfast"),
        entry(hm(8, 0), Decision::Work(places.workbench), "morning work"),
        entry(hm(12, 0), Decision::Sit(places.table), "lunch"),
        entry(
            hm(13, 0),
            Decision::Work(places.workbench),
            "afternoon work",
        ),
        entry(hm(18, 0), Decision::Sit(places.sofa), "evening off"),
        entry(hm(20, 0), Decision::Idle, "wandering"),
        entry(DEFAULT_BEDTIME, Decision::Sleep, "sleeping"),
    ])
}

fn entry(minute: u16, decision: Decision, label: &str) -> ScheduleEntry {
    ScheduleEntry {
        minute,
        decision,
        label: label.to_string(),
    }
}

/// Is this minute inside the night, given a bedtime and a wake time? Handles
/// the wrap past midnight, which is the only interesting case.
pub fn in_sleep_window(minute: u16, bedtime: u16, wake: u16) -> bool {
    if bedtime == wake {
        return false;
    }
    if bedtime < wake {
        minute >= bedtime && minute < wake
    } else {
        minute >= bedtime || minute < wake
    }
}

/// Bedtime after energy has its say: [`EARLY_BEDTIME_MINUTES`] earlier once
/// the window is under [`LOW_ENERGY`], wrapping past midnight.
pub fn effective_bedtime(bedtime: u16, energy: u8) -> u16 {
    if energy_pct(energy) >= LOW_ENERGY {
        return bedtime;
    }
    let day = MINUTES_PER_DAY as u16;
    (bedtime + day - (EARLY_BEDTIME_MINUTES % day)) % day
}

/// The agenda's answer for this minute, or `None` when it is the same thing
/// the agent is already doing. Returning `None` is what keeps the mailbox
/// (depth 4) from filling up with the same decision ten times a second.
///
/// Zero energy always means `Sleep`: the account has no window left, so the
/// agent is out until dawn.
pub fn hint(
    schedule: &Schedule,
    minute: u16,
    energy: u8,
    previous: Option<&Decision>,
) -> Option<Decision> {
    let asleep = energy == 0
        || in_sleep_window(
            minute,
            effective_bedtime(schedule.bedtime, energy),
            schedule.wake,
        );
    let decision = if asleep {
        Decision::Sleep
    } else {
        schedule
            .current(minute)
            .map(|e| e.decision.clone())
            .unwrap_or(Decision::Idle)
    };
    if previous == Some(&decision) {
        None
    } else {
        Some(decision)
    }
}

/// Did the agent just cross into being tired? Once per crossing, so the yawn
/// does not repeat every tick.
pub fn crossed_into_tired(before: u8, after: u8) -> bool {
    before > ENERGY_TIRED && after <= ENERGY_TIRED
}

/// Did the account's window just refill? That is the dawn of plan §9.1.
pub fn crossed_into_dawn(before: u8, after: u8) -> bool {
    before <= ENERGY_TIRED && after > ENERGY_TIRED
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places() -> Places {
        Places {
            bed: ObjectId(1),
            workbench: ObjectId(2),
            table: ObjectId(3),
            sofa: ObjectId(4),
        }
    }

    #[test]
    fn assert_mirror_constants() {
        // These must stay equal to omniget_world::sim::routine. When the core
        // gains the `omniget-world` dependency this test becomes an
        // `assert_eq!` against the real constants and this comment goes away.
        assert_eq!(TICKS_PER_GAME_MINUTE, 8);
        assert_eq!(TICKS_PER_DAY, 11_520);
        assert_eq!(TICKS_PER_GAME_HOUR, 480);
        assert_eq!(ENERGY_FULL, 255);
        assert_eq!(ENERGY_TIRED, 64);
        assert_eq!(ENERGY_SPENT, 16);
    }

    #[test]
    fn minutes_and_days_wrap_like_the_world() {
        assert_eq!(minute_of_day(0), 0);
        assert_eq!(minute_of_day(8), 1);
        assert_eq!(minute_of_day(TICKS_PER_DAY), 0);
        assert_eq!(minute_of_day(TICKS_PER_DAY + 8 * 90), hm(1, 30));
        assert_eq!(day_of(TICKS_PER_DAY - 1), 0);
        assert_eq!(day_of(TICKS_PER_DAY), 1);
        assert_eq!(hhmm(hm(9, 5)), "09:05");
        assert_eq!(hhmm(0), "00:00");
        assert_eq!(hhmm(23 * 60 + 59), "23:59");
    }

    #[test]
    fn the_default_day_has_a_bedtime_and_a_wake() {
        let s = default_schedule(&places());
        assert_eq!(s.len(), 8);
        assert_eq!(s.bedtime(), hm(22, 0));
        assert_eq!(s.wake(), hm(7, 0));
        assert_eq!(s.current(hm(9, 0)).unwrap().label, "morning work");
        assert_eq!(s.current(hm(12, 30)).unwrap().label, "lunch");
        // Before the first entry of the day yesterday's last one still holds.
        assert_eq!(s.current(hm(3, 0)).unwrap().decision, Decision::Sleep);
    }

    #[test]
    fn a_schedule_without_sleep_falls_back_to_the_default_night() {
        let s = Schedule::new(vec![entry(hm(9, 0), Decision::Idle, "loafing")]);
        assert_eq!(s.bedtime(), DEFAULT_BEDTIME);
        assert_eq!(s.wake(), DEFAULT_WAKE);
        assert!(!Schedule::default().is_empty() || Schedule::default().current(0).is_none());
    }

    #[test]
    fn the_sleep_window_wraps_past_midnight() {
        assert!(in_sleep_window(hm(23, 0), hm(22, 0), hm(7, 0)));
        assert!(in_sleep_window(hm(2, 0), hm(22, 0), hm(7, 0)));
        assert!(!in_sleep_window(hm(7, 0), hm(22, 0), hm(7, 0)));
        assert!(!in_sleep_window(hm(12, 0), hm(22, 0), hm(7, 0)));
        // A daytime nap window does not wrap.
        assert!(in_sleep_window(hm(14, 0), hm(13, 0), hm(15, 0)));
        assert!(!in_sleep_window(hm(16, 0), hm(13, 0), hm(15, 0)));
        assert!(!in_sleep_window(hm(5, 0), hm(9, 0), hm(9, 0)));
    }

    #[test]
    fn low_energy_moves_bedtime_two_hours_earlier() {
        assert_eq!(effective_bedtime(hm(22, 0), ENERGY_FULL), hm(22, 0));
        assert_eq!(effective_bedtime(hm(22, 0), ENERGY_SPENT), hm(20, 0));
        // Wrapping: a 01:00 bedtime becomes 23:00 the day before.
        assert_eq!(effective_bedtime(hm(1, 0), ENERGY_SPENT), hm(23, 0));
    }

    #[test]
    fn the_agenda_decides_without_a_model() {
        let s = default_schedule(&places());
        assert_eq!(
            hint(&s, hm(9, 0), ENERGY_FULL, None),
            Some(Decision::Work(ObjectId(2)))
        );
        assert_eq!(
            hint(&s, hm(12, 10), ENERGY_FULL, None),
            Some(Decision::Sit(ObjectId(3)))
        );
        assert_eq!(
            hint(&s, hm(23, 0), ENERGY_FULL, None),
            Some(Decision::Sleep)
        );
    }

    #[test]
    fn the_agenda_stays_quiet_when_nothing_changed() {
        let s = default_schedule(&places());
        let previous = Decision::Work(ObjectId(2));
        assert_eq!(hint(&s, hm(9, 0), ENERGY_FULL, Some(&previous)), None);
        assert_eq!(hint(&s, hm(9, 1), ENERGY_FULL, Some(&previous)), None);
        assert!(hint(&s, hm(12, 5), ENERGY_FULL, Some(&previous)).is_some());
    }

    #[test]
    fn low_energy_sends_the_agent_to_bed_early() {
        let s = default_schedule(&places());
        // 20:30 is still the evening at full energy and already night at 10%.
        assert_ne!(
            hint(&s, hm(20, 30), ENERGY_FULL, None),
            Some(Decision::Sleep)
        );
        let low = (f32::from(ENERGY_FULL) * 0.10) as u8;
        assert_eq!(hint(&s, hm(20, 30), low, None), Some(Decision::Sleep));
    }

    #[test]
    fn zero_energy_sleeps_at_noon() {
        let s = default_schedule(&places());
        assert_eq!(hint(&s, hm(12, 0), 0, None), Some(Decision::Sleep));
        assert_eq!(hint(&s, hm(12, 0), 0, Some(&Decision::Sleep)), None);
    }

    #[test]
    fn an_empty_schedule_idles_instead_of_panicking() {
        let s = Schedule::default();
        assert_eq!(hint(&s, hm(12, 0), ENERGY_FULL, None), Some(Decision::Idle));
    }

    #[test]
    fn quota_becomes_energy() {
        assert_eq!(energy_from_capacity(&Capacity::with_quota(1.0), None), 255);
        assert_eq!(energy_from_capacity(&Capacity::with_quota(0.0), None), 0);
        assert_eq!(energy_from_capacity(&Capacity::unavailable(), None), 0);
        // A sliver of window left is never rounded down to "dead".
        assert_eq!(energy_from_capacity(&Capacity::with_quota(0.001), None), 1);
        let half = energy_from_capacity(&Capacity::with_quota(0.5), None);
        assert!((127..=128).contains(&half), "{half}");
        // No window at all: an unmetered key is never tired.
        assert_eq!(energy_from_capacity(&Capacity::default(), None), 255);
    }

    #[test]
    fn a_dollar_budget_becomes_energy_when_there_is_no_window() {
        let capacity = Capacity {
            budget_remaining_usd: Some(2.5),
            ..Capacity::default()
        };
        assert_eq!(energy_from_capacity(&capacity, Some(10.0)), 64);
        let spent = Capacity {
            budget_remaining_usd: Some(0.0),
            ..Capacity::default()
        };
        assert_eq!(energy_from_capacity(&spent, Some(10.0)), 0);
        assert_eq!(energy_from_capacity(&spent, None), 0);
    }

    #[test]
    fn moods_and_crossings() {
        assert_eq!(mood(255), Mood::Fine);
        assert_eq!(mood(ENERGY_TIRED), Mood::Tired);
        assert_eq!(mood(ENERGY_SPENT), Mood::Spent);
        assert_eq!(mood(0), Mood::Exhausted);
        assert!(crossed_into_tired(65, 64));
        assert!(!crossed_into_tired(64, 60), "only the crossing, once");
        assert!(crossed_into_dawn(0, 255));
        assert!(!crossed_into_dawn(200, 255));
    }
}
