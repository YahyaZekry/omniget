//! The decision mailbox: the one door between the language model and the tick.
//!
//! The brain writes, the tick reads. Never the other way round. A model call
//! takes seconds and can fail; a tick takes two milliseconds and cannot. So
//! the brain drops a `Decision` in here whenever it finishes thinking, and the
//! tick picks up at most one per agent per tick. Nothing in this file blocks,
//! allocates per tick, or knows that a model exists.

use std::collections::BTreeMap;

use crate::ents::id::{EntId, ObjectId};
use crate::map::Tile;

/// Longest line an agent may say, in bytes. A model that writes an essay gets
/// it cut here and not in the renderer.
pub const MAX_SAY_BYTES: usize = 200;
/// Decisions kept per agent. Past this the oldest is dropped: a backlog of
/// stale intentions is worse than none.
pub const MAILBOX_DEPTH: usize = 4;

/// What an agent decided to do next.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Decision {
    GoTo(Tile),
    Sit(ObjectId),
    Sleep,
    Work(ObjectId),
    /// Truncated to [`MAX_SAY_BYTES`] on the way in, on a char boundary.
    Say(String),
    Wave(EntId),
    Idle,
}

impl Decision {
    /// A tag for the log and for the wire; the decision itself never travels,
    /// only its effect does.
    pub const fn tag(&self) -> u8 {
        match self {
            Decision::GoTo(_) => 0,
            Decision::Sit(_) => 1,
            Decision::Sleep => 2,
            Decision::Work(_) => 3,
            Decision::Say(_) => 4,
            Decision::Wave(_) => 5,
            Decision::Idle => 6,
        }
    }

    /// Cut an over-long `Say` down to size without splitting a character.
    pub fn sanitised(self) -> Decision {
        match self {
            Decision::Say(mut s) => {
                if s.len() > MAX_SAY_BYTES {
                    let mut end = MAX_SAY_BYTES;
                    while end > 0 && !s.is_char_boundary(end) {
                        end -= 1;
                    }
                    s.truncate(end);
                }
                Decision::Say(s)
            }
            other => other,
        }
    }
}

/// Pending decisions, one queue per agent, in id order.
#[derive(Clone, Debug, Default)]
pub struct Mailbox {
    queues: BTreeMap<EntId, Vec<Decision>>,
    /// Decisions thrown away because a queue was full. Reported by the step so
    /// the brain can slow down instead of guessing.
    pub dropped: u32,
}

impl Mailbox {
    pub fn new() -> Mailbox {
        Mailbox::default()
    }

    /// Post a decision. Full queues drop their oldest entry, not the new one:
    /// the newest intention is the one that reflects the world as it is.
    pub fn post(&mut self, ent: EntId, decision: Decision) {
        let q = self.queues.entry(ent).or_default();
        if q.len() >= MAILBOX_DEPTH {
            q.remove(0);
            self.dropped = self.dropped.saturating_add(1);
        }
        q.push(decision.sanitised());
    }

    /// Take the oldest decision for an agent, if any.
    pub fn take(&mut self, ent: EntId) -> Option<Decision> {
        let q = self.queues.get_mut(&ent)?;
        if q.is_empty() {
            return None;
        }
        Some(q.remove(0))
    }

    pub fn pending(&self, ent: EntId) -> usize {
        self.queues.get(&ent).map(|q| q.len()).unwrap_or(0)
    }

    pub fn total_pending(&self) -> usize {
        self.queues.values().map(|q| q.len()).sum()
    }

    /// Drop everything queued for an agent that just left the world.
    pub fn forget(&mut self, ent: EntId) {
        self.queues.remove(&ent);
    }

    pub fn clear(&mut self) {
        self.queues.clear();
        self.dropped = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_per_agent() {
        let mut m = Mailbox::new();
        m.post(EntId(1), Decision::Idle);
        m.post(EntId(1), Decision::Sleep);
        m.post(EntId(2), Decision::Wave(EntId(1)));
        assert_eq!(m.take(EntId(1)), Some(Decision::Idle));
        assert_eq!(m.take(EntId(1)), Some(Decision::Sleep));
        assert_eq!(m.take(EntId(1)), None);
        assert_eq!(m.take(EntId(2)), Some(Decision::Wave(EntId(1))));
        assert_eq!(m.take(EntId(3)), None);
    }

    #[test]
    fn a_full_queue_drops_the_oldest() {
        let mut m = Mailbox::new();
        for i in 0..MAILBOX_DEPTH + 2 {
            m.post(EntId(1), Decision::GoTo(Tile::new(i as i32, 0)));
        }
        assert_eq!(m.pending(EntId(1)), MAILBOX_DEPTH);
        assert_eq!(m.dropped, 2);
        assert_eq!(m.take(EntId(1)), Some(Decision::GoTo(Tile::new(2, 0))));
    }

    #[test]
    fn say_is_cut_on_a_char_boundary() {
        let long = "á".repeat(300); // 600 bytes
        let Decision::Say(s) = Decision::Say(long).sanitised() else {
            panic!("wrong variant");
        };
        assert!(s.len() <= MAX_SAY_BYTES);
        assert!(s.is_char_boundary(s.len()));
        assert_eq!(s.chars().count(), 100);
    }

    #[test]
    fn a_short_say_is_untouched() {
        let d = Decision::Say("oi".into()).sanitised();
        assert_eq!(d, Decision::Say("oi".into()));
    }

    #[test]
    fn posting_sanitises() {
        let mut m = Mailbox::new();
        m.post(EntId(1), Decision::Say("x".repeat(500)));
        let Some(Decision::Say(s)) = m.take(EntId(1)) else {
            panic!("wrong variant");
        };
        assert_eq!(s.len(), MAX_SAY_BYTES);
    }

    #[test]
    fn forgetting_an_agent_empties_its_queue() {
        let mut m = Mailbox::new();
        m.post(EntId(1), Decision::Idle);
        m.post(EntId(2), Decision::Idle);
        m.forget(EntId(1));
        assert_eq!(m.pending(EntId(1)), 0);
        assert_eq!(m.total_pending(), 1);
        m.clear();
        assert_eq!(m.total_pending(), 0);
        assert_eq!(m.dropped, 0);
    }

    #[test]
    fn tags_are_distinct() {
        let all = [
            Decision::GoTo(Tile::new(0, 0)),
            Decision::Sit(ObjectId(1)),
            Decision::Sleep,
            Decision::Work(ObjectId(1)),
            Decision::Say(String::new()),
            Decision::Wave(EntId(1)),
            Decision::Idle,
        ];
        let mut tags: Vec<u8> = all.iter().map(|d| d.tag()).collect();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), all.len());
    }
}
