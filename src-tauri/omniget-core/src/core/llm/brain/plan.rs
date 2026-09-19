//! Planning: a model's answer becomes a day the tick can run. Owned by
//! f7-world-brain.
//!
//! The parser is deliberately forgiving in the ways models actually fail
//! (numbered lists, a stray `**`, a missing object column, "9:00" for
//! "09:00", a sentence of preamble) and unforgiving in the way that matters:
//! a line it cannot read is skipped, never guessed at, and a plan with no
//! readable line at all is an error, so the caller falls back to the default
//! agenda instead of an agent standing still all day.
//!
//! Nothing here calls a model. `prompt::plan_prompt` writes the question and
//! this reads the answer; the two are one contract, tested from both ends.

use std::collections::BTreeMap;

use super::prompt::MAX_PLAN_LINES;
use super::schedule::{hm, Places, Schedule, ScheduleEntry};
use super::{BrainError, Decision, ObjectId};

/// One line of a plan, before it becomes a schedule entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    /// Minute of the game day, 0..1440.
    pub minute: u16,
    pub object: Option<ObjectId>,
    /// What the agent is doing, in its own words. Shown in the HUD.
    pub label: String,
}

/// Words that mean "sit down and do nothing productive".
const SITTING: [&str; 9] = [
    "lunch",
    "dinner",
    "breakfast",
    "eat",
    "sit",
    "rest",
    "read",
    "tea",
    "coffee",
];
/// Words that mean the day is over.
const SLEEPING: [&str; 4] = ["sleep", "bed", "sleeping", "asleep"];

/// Object names the house knows, lowercased, mapping to the ids the world
/// uses. Built by the bridge from `house-v1.json`.
pub type ObjectIndex = BTreeMap<String, ObjectId>;

/// Build an index from the `Places` an agent was given, so a plan works even
/// before the full house catalogue is wired.
pub fn index_from_places(places: &Places) -> ObjectIndex {
    let mut index = ObjectIndex::new();
    index.insert("bed".into(), places.bed);
    index.insert("workbench".into(), places.workbench);
    index.insert("table".into(), places.table);
    index.insert("sofa".into(), places.sofa);
    index
}

/// `"9:05"`, `"09:05"` and `"9h05"` all mean the same minute. Anything else
/// is `None`.
pub fn parse_time(token: &str) -> Option<u16> {
    let token = token.trim().trim_matches(|c: char| c == '`' || c == '*');
    let (h, m) = token
        .split_once(':')
        .or_else(|| token.split_once('h'))
        .or_else(|| token.split_once('.'))?;
    let h: u16 = h.trim().parse().ok()?;
    let m: u16 = m.trim().get(..2)?.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(hm(h, m))
}

fn strip_ornament(line: &str) -> &str {
    let line = line.trim();
    let line = line.trim_start_matches(['-', '*', '•', '#', '>']).trim();
    // "1. 08:00 | …" and "1) 08:00 | …"
    match line.split_once(['.', ')']) {
        Some((head, rest)) if !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()) => {
            rest.trim()
        }
        _ => line,
    }
}

/// Read one line of a plan. `None` for a line that is not a plan line —
/// preamble, a blank, a header, a sentence.
pub fn parse_line(line: &str, objects: &ObjectIndex) -> Option<PlanStep> {
    let line = strip_ornament(line);
    if line.is_empty() {
        return None;
    }
    let mut parts = line.split('|').map(str::trim);
    let minute = parse_time(parts.next()?)?;
    let rest: Vec<&str> = parts.collect();
    let (object_token, label) = match rest.as_slice() {
        [object, label] => (Some(*object), *label),
        [label] => (None, *label),
        _ => (None, ""),
    };
    let label = label.trim().trim_matches('*').trim();
    if label.is_empty() {
        return None;
    }
    let object = object_token
        .map(|t| t.trim().trim_matches(['*', '`']).to_lowercase())
        .filter(|t| !t.is_empty() && t != "-" && t != "none" && t != "nothing")
        .and_then(|t| lookup(&t, objects))
        // The object column is optional: "08:00 | fixing the workbench radio"
        // still finds the workbench.
        .or_else(|| lookup(&label.to_lowercase(), objects));
    Some(PlanStep {
        minute,
        object,
        label: label.to_string(),
    })
}

/// Exact name first, then any known name contained in the text. Longest name
/// wins, so "coffee table" beats "table".
fn lookup(text: &str, objects: &ObjectIndex) -> Option<ObjectId> {
    if let Some(id) = objects.get(text) {
        return Some(*id);
    }
    objects
        .iter()
        .filter(|(name, _)| text.contains(name.as_str()))
        .max_by_key(|(name, _)| name.len())
        .map(|(_, id)| *id)
}

/// Read a whole answer. Lines that are not plan lines are skipped; the result
/// is sorted by minute, deduplicated by minute (last line for a minute wins)
/// and capped at [`MAX_PLAN_LINES`].
pub fn parse_plan(answer: &str, objects: &ObjectIndex) -> Result<Vec<PlanStep>, BrainError> {
    let mut by_minute: BTreeMap<u16, PlanStep> = BTreeMap::new();
    for line in answer.lines() {
        if let Some(step) = parse_line(line, objects) {
            by_minute.insert(step.minute, step);
        }
    }
    if by_minute.is_empty() {
        return Err(BrainError::plan("no readable plan line in the answer"));
    }
    Ok(by_minute.into_values().take(MAX_PLAN_LINES).collect())
}

/// What a step means to the world. The label decides: sleeping words make a
/// `Sleep`, sitting words make a `Sit` on the named object, anything with an
/// object is `Work`, and anything else is `Idle`.
pub fn decision_for(step: &PlanStep) -> Decision {
    let label = step.label.to_lowercase();
    let tokens: Vec<&str> = label
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let has = |words: &[&str]| tokens.iter().any(|t| words.contains(t));
    if has(&SLEEPING) {
        return Decision::Sleep;
    }
    match step.object {
        Some(object) if has(&SITTING) => Decision::Sit(object),
        Some(object) => Decision::Work(object),
        None => Decision::Idle,
    }
}

/// Steps become the agenda the tick reads.
pub fn to_schedule(steps: &[PlanStep]) -> Schedule {
    Schedule::new(
        steps
            .iter()
            .map(|s| ScheduleEntry {
                minute: s.minute,
                decision: decision_for(s),
                label: s.label.clone(),
            })
            .collect(),
    )
}

/// Parse an answer into an agenda, falling back to the default day when the
/// model produced nothing usable. The `bool` says whether the model's plan
/// was used, so the caller can log it and the HUD can show "improvising".
pub fn schedule_from_answer(
    answer: &str,
    objects: &ObjectIndex,
    places: &Places,
) -> (Schedule, bool) {
    match parse_plan(answer, objects) {
        Ok(steps) => (to_schedule(&steps), true),
        Err(_) => (super::schedule::default_schedule(places), false),
    }
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

    fn objects() -> ObjectIndex {
        let mut index = index_from_places(&places());
        index.insert("coffee table".into(), ObjectId(9));
        index
    }

    #[test]
    fn times_are_read_in_the_shapes_models_write_them() {
        assert_eq!(parse_time("08:00"), Some(hm(8, 0)));
        assert_eq!(parse_time("8:05"), Some(hm(8, 5)));
        assert_eq!(parse_time(" 23:59 "), Some(hm(23, 59)));
        assert_eq!(parse_time("`07:30`"), Some(hm(7, 30)));
        assert_eq!(parse_time("9h15"), Some(hm(9, 15)));
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("08:61"), None);
        assert_eq!(parse_time("morning"), None);
        assert_eq!(parse_time(""), None);
    }

    #[test]
    fn a_plain_line_parses() {
        let step = parse_line("08:00 | workbench | fixing the radio", &objects()).unwrap();
        assert_eq!(step.minute, hm(8, 0));
        assert_eq!(step.object, Some(ObjectId(2)));
        assert_eq!(step.label, "fixing the radio");
        assert_eq!(decision_for(&step), Decision::Work(ObjectId(2)));
    }

    #[test]
    fn ornaments_and_numbering_are_stripped() {
        let o = objects();
        for line in [
            "- 08:00 | workbench | fixing the radio",
            "1. 08:00 | workbench | fixing the radio",
            "2) 08:00 | workbench | fixing the radio",
            "* **08:00** | workbench | fixing the radio",
        ] {
            let step = parse_line(line, &o).unwrap_or_else(|| panic!("failed on {line}"));
            assert_eq!(step.minute, hm(8, 0), "{line}");
            assert_eq!(step.object, Some(ObjectId(2)), "{line}");
        }
    }

    #[test]
    fn the_object_column_is_optional_and_can_hide_in_the_label() {
        let o = objects();
        let step = parse_line("12:30 | - | staring out of the window", &o).unwrap();
        assert_eq!(step.object, None);
        assert_eq!(decision_for(&step), Decision::Idle);

        let step = parse_line("09:00 | tidying up the workbench", &o).unwrap();
        assert_eq!(step.object, Some(ObjectId(2)));

        let step = parse_line("13:00 | none | thinking", &o).unwrap();
        assert_eq!(step.object, None);
    }

    #[test]
    fn the_longest_object_name_wins() {
        let step = parse_line("15:00 | coffee table | tea", &objects()).unwrap();
        assert_eq!(step.object, Some(ObjectId(9)));
        assert_eq!(decision_for(&step), Decision::Sit(ObjectId(9)));
        // "reading" is not "read": the word list matches whole tokens only.
        let reading = parse_line("16:00 | coffee table | reading", &objects()).unwrap();
        assert_eq!(decision_for(&reading), Decision::Work(ObjectId(9)));
    }

    #[test]
    fn sitting_sleeping_and_working_are_told_apart() {
        let o = objects();
        let lunch = parse_line("12:30 | table | lunch", &o).unwrap();
        assert_eq!(decision_for(&lunch), Decision::Sit(ObjectId(3)));
        let bed = parse_line("22:00 | bed | sleep", &o).unwrap();
        assert_eq!(decision_for(&bed), Decision::Sleep);
        let work = parse_line("08:00 | workbench | soldering", &o).unwrap();
        assert_eq!(decision_for(&work), Decision::Work(ObjectId(2)));
        // A sleeping word anywhere in the label wins, even over an object:
        // the rule is crude on purpose and this pins it.
        let ambiguous = parse_line("10:00 | workbench | sorting sleep masks", &o).unwrap();
        assert_eq!(decision_for(&ambiguous), Decision::Sleep);
        let plain = parse_line("10:00 | workbench | sorting the masks", &o).unwrap();
        assert_eq!(decision_for(&plain), Decision::Work(ObjectId(2)));
    }

    #[test]
    fn junk_lines_are_skipped_not_guessed() {
        let o = objects();
        assert!(parse_line("", &o).is_none());
        assert!(parse_line("Here is my plan for today:", &o).is_none());
        assert!(parse_line("08:00 |  |  ", &o).is_none());
        assert!(parse_line("nonsense | workbench | working", &o).is_none());
    }

    #[test]
    fn a_whole_answer_parses_with_preamble_and_a_repeat() {
        let answer = "Sure! Here is my day:\n\
                      \n\
                      07:30 | table | breakfast\n\
                      08:00 | workbench | fixing the radio\n\
                      08:00 | workbench | actually, rewiring the radio\n\
                      12:30 | table | lunch\n\
                      22:00 | bed | sleep\n\
                      Let me know if you want changes.";
        let steps = parse_plan(answer, &objects()).unwrap();
        assert_eq!(steps.len(), 4, "the repeated minute collapses");
        assert_eq!(steps[0].minute, hm(7, 30));
        assert_eq!(steps[1].label, "actually, rewiring the radio");
        assert_eq!(steps[3].minute, hm(22, 0));
    }

    #[test]
    fn an_answer_with_nothing_readable_is_an_error() {
        let err = parse_plan("I would rather not.", &objects()).unwrap_err();
        assert_eq!(err.code, super::super::ERR_BRAIN_PLAN);
        assert!(parse_plan("", &objects()).is_err());
    }

    #[test]
    fn a_long_answer_is_capped() {
        let mut answer = String::new();
        for i in 0..60 {
            answer.push_str(&format!("{:02}:{:02} | - | thing {i}\n", i % 24, i % 60));
        }
        let steps = parse_plan(&answer, &objects()).unwrap();
        assert!(steps.len() <= MAX_PLAN_LINES, "{}", steps.len());
    }

    #[test]
    fn a_parsed_plan_becomes_an_agenda_the_tick_can_run() {
        let answer = "07:30 | table | breakfast\n\
                      08:00 | workbench | fixing the radio\n\
                      22:00 | bed | sleep";
        let (schedule, used) = schedule_from_answer(answer, &objects(), &places());
        assert!(used);
        assert_eq!(schedule.len(), 3);
        assert_eq!(schedule.bedtime(), hm(22, 0));
        assert_eq!(schedule.wake(), hm(7, 30));
        assert_eq!(
            schedule.current(hm(9, 0)).unwrap().decision,
            Decision::Work(ObjectId(2))
        );
    }

    #[test]
    fn a_useless_answer_falls_back_to_the_default_day() {
        let (schedule, used) = schedule_from_answer("no thanks", &objects(), &places());
        assert!(!used);
        assert_eq!(schedule.len(), 8);
        assert_eq!(schedule.bedtime(), hm(22, 0));
    }
}
