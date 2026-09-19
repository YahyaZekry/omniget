//! Reflection: a day of observations becomes a handful of durable beliefs.
//! Owned by f7-world-brain.
//!
//! Once per game day per agent, by the budget in the prompt §5 — a day is
//! 19.2 real minutes, so a machine left open all afternoon spends one call
//! per agent per 19 minutes and nothing else. The consolidation is the whole
//! point: 60 observations go in, at most [`MAX_REFLECTIONS`] sentences come
//! out, and the observations can then be pruned.

use super::memory::{Memory, MemoryKind, MAX_IMPORTANCE};
use super::observe::Observation;
use super::prompt::MAX_REFLECTIONS;

/// A day with fewer observations than this is not worth a model call.
pub const MIN_OBSERVATIONS: usize = 3;
/// Observations handed to one reflection prompt, at most.
pub const REFLECT_WINDOW: usize = 20;

/// Is this agent due a reflection? One per game day, never on an empty day,
/// never twice — the day counter is the lock.
pub fn should_reflect(day: u64, last_reflection_day: u64, observations: usize) -> bool {
    day > last_reflection_day && observations >= MIN_OBSERVATIONS
}

/// The observations a reflection prompt gets: the most important ones, ties
/// broken by the most recent, handed over in the order they happened so the
/// model reads a day rather than a leaderboard.
pub fn select_for_reflection(observations: &[Observation], max: usize) -> Vec<Observation> {
    let max = max.min(REFLECT_WINDOW);
    let mut ordered: Vec<(usize, &Observation)> = observations.iter().enumerate().collect();
    ordered.sort_by(|(ai, a), (bi, b)| {
        b.importance
            .cmp(&a.importance)
            .then_with(|| b.tick.cmp(&a.tick))
            .then_with(|| ai.cmp(bi))
    });
    ordered.truncate(max);
    ordered.sort_by_key(|(i, _)| *i);
    ordered.into_iter().map(|(_, o)| o.clone()).collect()
}

/// Read a reflection answer: `<importance> | <sentence>` per line. Junk lines
/// are skipped, duplicates collapse, and the result is capped at
/// [`MAX_REFLECTIONS`].
pub fn parse_reflections(answer: &str) -> Vec<(u8, String)> {
    let mut out: Vec<(u8, String)> = Vec::new();
    for line in answer.lines() {
        let line = line
            .trim()
            .trim_start_matches(['-', '*', '•', '#', '>'])
            .trim();
        let Some((head, text)) = line.split_once('|') else {
            continue;
        };
        let head = head.trim().trim_matches(|c: char| !c.is_ascii_digit());
        let Ok(importance) = head.parse::<u8>() else {
            continue;
        };
        let text = text.trim().trim_matches('*').trim();
        if text.is_empty() {
            continue;
        }
        if out.iter().any(|(_, t)| t.eq_ignore_ascii_case(text)) {
            continue;
        }
        out.push((importance.clamp(1, MAX_IMPORTANCE), text.to_string()));
        if out.len() == MAX_REFLECTIONS {
            break;
        }
    }
    out
}

/// Parsed reflections become memories of kind `Reflection`, ready to store.
pub fn into_memories(agent_id: &str, parsed: &[(u8, String)], tick: u64) -> Vec<Memory> {
    parsed
        .iter()
        .map(|(importance, text)| {
            Memory::new(agent_id, MemoryKind::Reflection, text, *importance, tick)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::observe::Source;
    use super::*;

    fn observation(text: &str, importance: u8, tick: u64) -> Observation {
        Observation::new(text, importance, tick, Source::World)
    }

    #[test]
    fn reflection_happens_once_a_day_and_not_on_an_empty_day() {
        assert!(should_reflect(1, 0, 10));
        assert!(!should_reflect(1, 1, 10), "already reflected today");
        assert!(!should_reflect(0, 0, 10), "day zero has no yesterday");
        assert!(!should_reflect(3, 2, 2), "too little happened");
        assert!(
            should_reflect(9, 2, MIN_OBSERVATIONS),
            "a skipped day still counts"
        );
    }

    #[test]
    fn selection_keeps_the_important_ones_in_chronological_order() {
        let observations = vec![
            observation("woke up", 1, 0),
            observation("the tool failed", 7, 10),
            observation("grace said hello", 4, 20),
            observation("walked around", 1, 30),
            observation("the house went dark", 9, 40),
        ];
        let picked = select_for_reflection(&observations, 3);
        assert_eq!(picked.len(), 3);
        assert_eq!(picked[0].text, "the tool failed");
        assert_eq!(picked[1].text, "grace said hello");
        assert_eq!(picked[2].text, "the house went dark");
        assert!(picked.windows(2).all(|w| w[0].tick <= w[1].tick));
    }

    #[test]
    fn selection_is_bounded_and_deterministic() {
        let observations: Vec<Observation> = (0..100).map(|i| observation("same", 5, i)).collect();
        let a = select_for_reflection(&observations, 1000);
        let b = select_for_reflection(&observations, 1000);
        assert_eq!(a.len(), REFLECT_WINDOW);
        assert_eq!(a, b);
        assert!(select_for_reflection(&[], 5).is_empty());
    }

    #[test]
    fn a_reflection_answer_parses() {
        let answer = "Here is what I think:\n\
                      8 | the workbench breaks when I rush\n\
                      - 4 | Grace is usually in the kitchen in the morning\n\
                      **3** | the sofa is the quiet corner";
        let got = parse_reflections(answer);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], (8, "the workbench breaks when I rush".to_string()));
        assert_eq!(got[1].0, 4);
        assert_eq!(got[2].0, 3);
    }

    #[test]
    fn junk_duplicates_and_extra_lines_are_dropped() {
        let answer = "no idea\n\
                      7 | the kettle is always on\n\
                      7 | The kettle is always on\n\
                      x | broken importance\n\
                      9 |   \n\
                      2 | one\n3 | two\n4 | three\n5 | four\n6 | five\n7 | six";
        let got = parse_reflections(answer);
        assert_eq!(got.len(), MAX_REFLECTIONS);
        assert_eq!(got[0].1, "the kettle is always on");
        assert!(!got.iter().any(|(_, t)| t == "broken importance"));
    }

    #[test]
    fn importance_is_clamped_to_the_scale() {
        let got = parse_reflections("99 | too loud\n0 | too quiet");
        assert_eq!(got[0].0, MAX_IMPORTANCE);
        assert_eq!(got[1].0, 1);
    }

    #[test]
    fn reflections_become_memories() {
        let parsed = parse_reflections("6 | the radio needs a new valve");
        let memories = into_memories("ada", &parsed, 5_000);
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].kind, MemoryKind::Reflection);
        assert_eq!(memories[0].agent_id, "ada");
        assert_eq!(memories[0].importance, 6);
        assert_eq!(memories[0].created_tick, 5_000);
        assert!(memories[0].embedding.is_none(), "embedded in a batch later");
    }
}
