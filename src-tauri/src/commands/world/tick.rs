//! Tick-loop policy. Owned by f7-world-bridge.
//!
//! Everything the `omniget-world-tick` thread decides is a pure function in
//! this file, so the thread itself stays a `loop { lock; act; wait }` with
//! nothing in it worth testing. Three decisions live here:
//!
//! 1. **Which sleep state applies** ([`decide_sleep`]). Active at 10 Hz with a
//!    visible route, Dozing at 0.2 Hz with the route closed, Hibernating at
//!    0 Hz once the app has been in the background for more than ten minutes.
//! 2. **What one wake-up does** ([`plan_for`]). Active steps once; Dozing steps
//!    once and catches the other 4.9 s up in bulk, which is the whole reason
//!    `World::catch_up` exists; Hibernating does nothing at all.
//! 3. **What to do when the webview is not draining the channel**
//!    ([`Outbox`]). The tick must never block on the IPC, so the queue is
//!    bounded, the oldest blob is dropped, and the drop arms a re-sync: the
//!    next thing the client gets is a whole snapshot, not a diff it cannot
//!    apply (`World::apply` refuses a gap with `ERR_WORLD_DIFF_GAP`).

use std::collections::VecDeque;

use omniget_world::{SleepState, TICK_MS};

/// Time in the background after which the world stops running entirely. The
/// plan's number (§2.3): ten minutes minimised.
pub const HIBERNATE_AFTER_MS: u64 = 10 * 60 * 1000;

/// Blobs the bridge will hold for a webview that is not draining the channel.
/// Sixteen frames is 1.6 s at 10 Hz — long enough to ride out a garbage
/// collection, short enough that a stalled page costs a re-sync instead of
/// unbounded memory.
pub const OUTBOX_CAP: usize = 16;

/// Which sleep state the world should be in.
///
/// Background time wins over route visibility: a minimised app draws nothing,
/// so a route that is nominally "visible" behind it still hibernates.
pub fn decide_sleep(route_visible: bool, background_for_ms: Option<u64>) -> SleepState {
    if let Some(ms) = background_for_ms {
        if ms >= HIBERNATE_AFTER_MS {
            return SleepState::Hibernating;
        }
    }
    if route_visible {
        SleepState::Active
    } else {
        SleepState::Dozing
    }
}

/// Stable string for the `world://sleep-state` event.
pub const fn sleep_tag(s: SleepState) -> &'static str {
    match s {
        SleepState::Active => "active",
        SleepState::Dozing => "dozing",
        SleepState::Hibernating => "hibernating",
    }
}

/// What one wake-up of the tick thread does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TickPlan {
    /// Nothing runs.
    Idle,
    /// One `step()`.
    Step,
    /// One `step()`, then the rest of the interval skipped in bulk.
    StepThenCatchUp { ms: u64 },
}

/// The plan for a state, derived from the crate's own `interval_ms`/`stride`
/// so the two can never disagree.
pub fn plan_for(s: SleepState) -> TickPlan {
    match s.interval_ms() {
        None => TickPlan::Idle,
        Some(interval) if interval <= TICK_MS => TickPlan::Step,
        Some(interval) => TickPlan::StepThenCatchUp {
            ms: interval - TICK_MS,
        },
    }
}

/// How long to wait before the next wake-up, or `None` to park until something
/// notifies the thread.
pub fn wait_ms(s: SleepState) -> Option<u64> {
    s.interval_ms()
}

/// Bounded outbound queue for the binary channel.
///
/// `push` never blocks and never grows past `cap`. When it has to drop, it
/// drops the *oldest* blob — a client that is behind wants the newest state,
/// not the oldest — and raises [`Outbox::needs_resync`], because a diff stream
/// with a hole in it is worse than no diff at all.
#[derive(Debug)]
pub struct Outbox {
    queue: VecDeque<Vec<u8>>,
    cap: usize,
    sent: u64,
    dropped: u64,
    resyncs: u64,
    bytes: u64,
    resync: bool,
}

impl Default for Outbox {
    fn default() -> Self {
        Outbox::new(OUTBOX_CAP)
    }
}

impl Outbox {
    pub fn new(cap: usize) -> Outbox {
        Outbox {
            queue: VecDeque::with_capacity(cap.min(64)),
            cap: cap.max(1),
            sent: 0,
            dropped: 0,
            resyncs: 0,
            bytes: 0,
            resync: false,
        }
    }

    /// Queue a blob, dropping the oldest one if the queue is full.
    pub fn push(&mut self, blob: Vec<u8>) {
        while self.queue.len() >= self.cap {
            self.queue.pop_front();
            self.dropped += 1;
            self.arm_resync();
        }
        self.queue.push_back(blob);
    }

    /// Everything queued, in order, leaving the queue empty. The caller sends
    /// these with the state lock released.
    pub fn take(&mut self) -> Vec<Vec<u8>> {
        let out: Vec<Vec<u8>> = self.queue.drain(..).collect();
        self.sent += out.len() as u64;
        self.bytes += out.iter().map(|b| b.len() as u64).sum::<u64>();
        out
    }

    /// The client's view has a hole in it; the next thing it gets must be a
    /// whole snapshot.
    pub fn arm_resync(&mut self) {
        if !self.resync {
            self.resync = true;
            self.resyncs += 1;
        }
    }

    pub const fn needs_resync(&self) -> bool {
        self.resync
    }

    /// Called once the snapshot that closes the hole has been queued.
    pub fn clear_resync(&mut self) {
        self.resync = false;
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub const fn sent(&self) -> u64 {
        self.sent
    }

    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    pub const fn resyncs(&self) -> u64 {
        self.resyncs
    }

    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Forget the backlog without counting it as a drop: used when the channel
    /// itself goes away, where there is nobody left to be out of sync.
    pub fn reset(&mut self) {
        self.queue.clear();
        self.resync = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_visible_route_runs_at_ten_hertz() {
        let s = decide_sleep(true, None);
        assert_eq!(s, SleepState::Active);
        assert_eq!(s.interval_ms(), Some(100));
    }

    #[test]
    fn a_closed_route_dozes_at_a_fifth_of_a_hertz() {
        let s = decide_sleep(false, None);
        assert_eq!(s, SleepState::Dozing);
        assert_eq!(s.interval_ms(), Some(5_000));
    }

    #[test]
    fn ten_minutes_in_the_background_hibernates_even_with_the_route_open() {
        assert_eq!(
            decide_sleep(true, Some(HIBERNATE_AFTER_MS - 1)),
            SleepState::Active
        );
        assert_eq!(
            decide_sleep(true, Some(HIBERNATE_AFTER_MS)),
            SleepState::Hibernating
        );
        assert_eq!(
            decide_sleep(false, Some(HIBERNATE_AFTER_MS + 1)),
            SleepState::Hibernating
        );
        assert_eq!(wait_ms(SleepState::Hibernating), None);
    }

    #[test]
    fn dozing_catches_up_exactly_the_gap_it_leaves() {
        assert_eq!(plan_for(SleepState::Active), TickPlan::Step);
        assert_eq!(plan_for(SleepState::Hibernating), TickPlan::Idle);
        let TickPlan::StepThenCatchUp { ms } = plan_for(SleepState::Dozing) else {
            panic!("dozing must catch up");
        };
        // One real tick plus the skipped remainder is the whole interval.
        assert_eq!(ms + TICK_MS, SleepState::Dozing.interval_ms().unwrap());
        assert_eq!(ms / TICK_MS + 1, SleepState::Dozing.stride());
    }

    #[test]
    fn sleep_tags_are_the_strings_the_route_switches_on() {
        assert_eq!(sleep_tag(SleepState::Active), "active");
        assert_eq!(sleep_tag(SleepState::Dozing), "dozing");
        assert_eq!(sleep_tag(SleepState::Hibernating), "hibernating");
    }

    #[test]
    fn the_outbox_keeps_order_and_counts_bytes() {
        let mut o = Outbox::new(4);
        o.push(vec![1, 2, 3]);
        o.push(vec![4]);
        assert_eq!(o.len(), 2);
        assert_eq!(o.take(), vec![vec![1, 2, 3], vec![4]]);
        assert!(o.is_empty());
        assert_eq!(o.sent(), 2);
        assert_eq!(o.bytes(), 4);
        assert!(!o.needs_resync());
    }

    #[test]
    fn a_full_outbox_drops_the_oldest_and_asks_for_a_resync() {
        let mut o = Outbox::new(2);
        o.push(vec![1]);
        o.push(vec![2]);
        o.push(vec![3]);
        assert_eq!(o.len(), 2, "never grows past the cap");
        assert_eq!(o.dropped(), 1);
        assert!(o.needs_resync(), "a hole in the stream needs a snapshot");
        assert_eq!(o.take(), vec![vec![2], vec![3]], "the newest survive");
        // The resync flag survives the drain; only sending the snapshot clears it.
        assert!(o.needs_resync());
        o.clear_resync();
        assert!(!o.needs_resync());
        assert_eq!(o.resyncs(), 1, "one hole, one resync");
    }

    #[test]
    fn many_drops_in_a_row_count_as_one_resync() {
        let mut o = Outbox::new(1);
        for i in 0..10u8 {
            o.push(vec![i]);
        }
        assert_eq!(o.dropped(), 9);
        assert_eq!(o.resyncs(), 1);
        o.clear_resync();
        o.push(vec![99]);
        o.push(vec![100]);
        assert_eq!(o.resyncs(), 2, "a new hole after a resync counts again");
    }

    #[test]
    fn resetting_forgets_the_backlog_without_a_resync() {
        let mut o = Outbox::new(2);
        o.push(vec![1]);
        o.arm_resync();
        o.reset();
        assert!(o.is_empty());
        assert!(!o.needs_resync());
    }
}
