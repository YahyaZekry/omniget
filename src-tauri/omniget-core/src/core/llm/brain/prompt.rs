//! Every prompt the brain sends, as a pure function of its inputs. Owned by
//! f7-world-brain.
//!
//! Pure on purpose: a prompt that is built by a function with no clock, no
//! randomness and no I/O can be snapshot-tested, and a change to the wording
//! shows up as a failing test instead of as a different-looking agent three
//! days later. The answers these ask for are parsed by `plan::parse_plan` and
//! `reflect::parse_reflections`; the format described here and the parser
//! there are one contract, tested from both ends.

use super::memory::{Memory, MAX_IMPORTANCE};
use super::observe::Observation;
use super::schedule::{hhmm, Schedule};

/// The world cuts `Say` at 200 bytes on a char boundary. The brain cuts it
/// the same way first, so nothing is silently truncated twice.
pub const MAX_SAY_BYTES: usize = 200;

/// Lines of a plan answer the parser will look at, at most. A model that
/// answers with an essay does not get to blow up the schedule.
pub const MAX_PLAN_LINES: usize = 24;
/// Reflections kept from one reflection turn.
pub const MAX_REFLECTIONS: usize = 5;

/// Cut a line of speech to [`MAX_SAY_BYTES`] without splitting a character.
pub fn clamp_say(text: &str) -> String {
    let text = text.trim();
    if text.len() <= MAX_SAY_BYTES {
        return text.to_string();
    }
    let mut end = MAX_SAY_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].trim_end().to_string()
}

/// What a tired agent says out loud. Deterministic: same tick, same line.
pub fn yawn_line(tick: u64) -> &'static str {
    const LINES: [&str; 4] = [
        "*yawns* I am running low, I should turn in.",
        "*rubs eyes* Almost out of steam for today.",
        "*yawns* My window is nearly spent.",
        "I am fading. Bed, I think.",
    ];
    LINES[(tick as usize) % LINES.len()]
}

/// The system line every brain prompt opens with.
pub fn persona(name: &str, role: &str) -> String {
    format!(
        "You are {name}, a {role} who lives in a small house with the other agents of this app. \
Answer in plain text, in the exact format asked for, with no preamble and no markdown."
    )
}

fn bullet_observations(observations: &[Observation]) -> String {
    let mut out = String::new();
    for o in observations {
        out.push_str(&format!("- ({}) {}\n", o.importance, o.text));
    }
    out
}

fn bullet_memories(memories: &[Memory]) -> String {
    let mut out = String::new();
    for m in memories {
        out.push_str(&format!("- {}\n", m.text));
    }
    out
}

/// The importance question: one number for one observation. Cheap and short
/// on purpose — it runs far more often than the other two.
pub fn rate_prompt(name: &str, text: &str) -> String {
    format!(
        "{}\n\nOn a scale of 1 to {MAX_IMPORTANCE}, where 1 is brushing your teeth and \
{MAX_IMPORTANCE} is the house burning down, how important is this to you?\n\n{}\n\n\
Answer with the number alone.",
        persona(name, "resident"),
        text
    )
}

/// The daily reflection: the day's observations in, a handful of durable
/// conclusions out. One call per game day per agent, by the budget.
pub fn reflect_prompt(name: &str, day: u64, observations: &[Observation]) -> String {
    format!(
        "{}\n\nThese things happened to you on day {day}:\n\n{}\n\
Write at most {MAX_REFLECTIONS} things you now believe, one per line, in the form\n\
`<importance 1-{MAX_IMPORTANCE}> | <one sentence>`.\n\
Write only what these events support. Do not repeat an event as if it were a conclusion.",
        persona(name, "resident"),
        bullet_observations(observations)
    )
}

/// The daily plan: memories in, an agenda out. The format is the one
/// `plan::parse_plan` accepts, and the example is part of the contract.
pub fn plan_prompt(
    name: &str,
    day: u64,
    energy_pct: f32,
    memories: &[Memory],
    objects: &[String],
    current: &Schedule,
) -> String {
    let energy = (energy_pct * 100.0).round().clamp(0.0, 100.0) as u32;
    let places = if objects.is_empty() {
        "nothing in particular".to_string()
    } else {
        objects.join(", ")
    };
    let bedtime = hhmm(current.bedtime());
    format!(
        "{}\n\nIt is the morning of day {day}. You have {energy}% of your energy left.\n\
What you remember:\n\n{}\n\
Things you can use in the house: {places}.\n\n\
Write your plan for today as at most {MAX_PLAN_LINES} lines, one per line, in the form\n\
`HH:MM | <object or ->| <what you are doing>`, in order, starting after you wake up and \
ending with a line whose activity is `sleep` at or before {bedtime}.\n\
Example:\n\
08:00 | workbench | fixing the radio\n\
12:30 | table | lunch\n\
22:00 | bed | sleep",
        persona(name, "resident"),
        bullet_memories(memories)
    )
}

#[cfg(test)]
mod tests {
    use super::super::memory::MemoryKind;
    use super::super::observe::Source;
    use super::super::schedule::{default_schedule, Places};
    use super::super::ObjectId;
    use super::*;

    fn observation(text: &str, importance: u8) -> Observation {
        Observation::new(text, importance, 0, Source::World)
    }

    fn memory(text: &str) -> Memory {
        Memory::new("ada", MemoryKind::Observation, text, 5, 0)
    }

    #[test]
    fn say_is_clamped_to_200_bytes_on_a_char_boundary() {
        assert_eq!(clamp_say("  hello  "), "hello");
        let long = "a".repeat(500);
        assert_eq!(clamp_say(&long).len(), MAX_SAY_BYTES);
        // 'é' is two bytes: cutting at 200 must not land inside it.
        let accented = "é".repeat(300);
        let cut = clamp_say(&accented);
        assert!(cut.len() <= MAX_SAY_BYTES);
        assert_eq!(cut.chars().count(), 100);
        assert!(std::str::from_utf8(cut.as_bytes()).is_ok());
        // An emoji at the boundary is kept whole or dropped whole.
        let emoji = format!("{}🙂", "a".repeat(199));
        let cut = clamp_say(&emoji);
        assert_eq!(cut.len(), 199);
    }

    #[test]
    fn the_yawn_is_deterministic() {
        assert_eq!(yawn_line(0), yawn_line(4));
        assert_ne!(yawn_line(0), yawn_line(1));
        assert!(yawn_line(2).len() < MAX_SAY_BYTES);
    }

    #[test]
    fn the_reflection_prompt_is_this_exact_text() {
        let got = reflect_prompt(
            "Ada",
            3,
            &[
                observation("Grace said \"the kettle is on\"", 4),
                observation("I ran the tool yt-dlp at the workbench (120 ms)", 5),
            ],
        );
        assert_eq!(
            got,
            "You are Ada, a resident who lives in a small house with the other agents of \
this app. Answer in plain text, in the exact format asked for, with no preamble and no \
markdown.\n\
\n\
These things happened to you on day 3:\n\
\n\
- (4) Grace said \"the kettle is on\"\n\
- (5) I ran the tool yt-dlp at the workbench (120 ms)\n\
\n\
Write at most 5 things you now believe, one per line, in the form\n\
`<importance 1-10> | <one sentence>`.\n\
Write only what these events support. Do not repeat an event as if it were a conclusion."
        );
    }

    #[test]
    fn the_plan_prompt_carries_energy_memories_and_the_house() {
        let schedule = default_schedule(&Places {
            bed: ObjectId(1),
            workbench: ObjectId(2),
            table: ObjectId(3),
            sofa: ObjectId(4),
        });
        let got = plan_prompt(
            "Ada",
            2,
            0.42,
            &[memory("the workbench needs a new saw")],
            &["workbench".into(), "table".into(), "bed".into()],
            &schedule,
        );
        assert!(got.contains("It is the morning of day 2. You have 42% of your energy left."));
        assert!(got.contains("- the workbench needs a new saw"));
        assert!(got.contains("Things you can use in the house: workbench, table, bed."));
        assert!(got.contains("ending with a line whose activity is `sleep` at or before 22:00"));
        assert!(got.contains("HH:MM | <object or ->| <what you are doing>"));
    }

    #[test]
    fn the_plan_prompt_survives_an_empty_house_and_no_memories() {
        let got = plan_prompt("Ada", 0, 0.0, &[], &[], &Schedule::default());
        assert!(got.contains("You have 0% of your energy left."));
        assert!(got.contains("Things you can use in the house: nothing in particular."));
        assert!(!got.contains("- \n"));
    }

    #[test]
    fn the_rating_prompt_asks_for_one_number() {
        let got = rate_prompt("Ada", "the kettle boiled");
        assert!(got.contains("On a scale of 1 to 10"));
        assert!(got.contains("the kettle boiled"));
        assert!(got.ends_with("Answer with the number alone."));
    }
}
