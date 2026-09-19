//! The judge seam. A judge answers two questions per `tool_use_id` — does the
//! call still matter, does its full output still matter — exactly the two
//! `noul` questions `tamaratran/fast-jev-compaction` asks
//! (`src/compact.ts` `questionsFor`).
//!
//! Fail-open is the contract, not an implementation detail: a judge that
//! errors, times out or answers only half the batch simply returns fewer
//! entries, and everything it did not answer is kept. No judge can ever cause
//! an omission by failing.

use std::collections::HashMap;

use async_trait::async_trait;

use super::policy::{Candidate, GoalContext, KeepVerdict};

/// Verdicts by `tool_use_id`. A missing id means keep.
pub type Verdicts = HashMap<String, KeepVerdict>;

#[async_trait]
pub trait ContextJudge: Send + Sync {
    /// Which judge this is, recorded in the sticky store.
    fn name(&self) -> &'static str;

    /// Judge the candidates against the goal. Implementations must never
    /// return a drop verdict for anything they are unsure about, and must
    /// return an empty map rather than an error when they cannot answer.
    async fn judge(&self, goal: &GoalContext, candidates: &[Candidate]) -> Verdicts;
}

/// The judge used when pruning is enabled but no judge is configured: it keeps
/// everything, so the wiring is exercised and nothing is lost.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeepAllJudge;

#[async_trait]
impl ContextJudge for KeepAllJudge {
    fn name(&self) -> &'static str {
        "keep_all"
    }

    async fn judge(&self, _goal: &GoalContext, _candidates: &[Candidate]) -> Verdicts {
        Verdicts::new()
    }
}

/// Turns a pair of keep probabilities into a verdict, with fast-jev's
/// `decideCall` ladder: the result outranks the call, because a call whose
/// output is still needed obviously still matters.
pub fn decide(keep_call: f32, keep_result: f32, threshold: f32) -> KeepVerdict {
    if keep_result >= threshold {
        KeepVerdict::KEEP
    } else if keep_call >= threshold {
        KeepVerdict::DROP_RESULT
    } else {
        KeepVerdict::DROP_CALL
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::Mutex;

    /// Answers from a script, and counts how often it was asked. `fail` makes
    /// it behave like a judge that errored: an empty map.
    pub struct FakeJudge {
        answers: Vec<(String, KeepVerdict)>,
        fail: bool,
        pub seen: Mutex<Vec<String>>,
        pub calls: Mutex<usize>,
    }

    impl FakeJudge {
        pub fn new(answers: Vec<(&str, KeepVerdict)>) -> Self {
            Self {
                answers: answers
                    .into_iter()
                    .map(|(id, v)| (id.to_string(), v))
                    .collect(),
                fail: false,
                seen: Mutex::new(Vec::new()),
                calls: Mutex::new(0),
            }
        }

        pub fn failing() -> Self {
            Self {
                answers: Vec::new(),
                fail: true,
                seen: Mutex::new(Vec::new()),
                calls: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl ContextJudge for FakeJudge {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn judge(&self, _goal: &GoalContext, candidates: &[Candidate]) -> Verdicts {
            *self.calls.lock().unwrap() += 1;
            let mut seen = self.seen.lock().unwrap();
            for candidate in candidates {
                seen.push(candidate.tool_use_id.clone());
            }
            if self.fail {
                return Verdicts::new();
            }
            let asked: Vec<&str> = candidates.iter().map(|c| c.tool_use_id.as_str()).collect();
            self.answers
                .iter()
                .filter(|(id, _)| asked.contains(&id.as_str()))
                .map(|(id, v)| (id.clone(), *v))
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_escada_de_decisao_e_a_do_fast_jev() {
        assert_eq!(decide(0.9, 0.9, 0.5), KeepVerdict::KEEP);
        assert_eq!(decide(0.9, 0.4, 0.5), KeepVerdict::DROP_RESULT);
        assert_eq!(decide(0.4, 0.4, 0.5), KeepVerdict::DROP_CALL);
        // Exactly at the threshold keeps, like `>=` in the original.
        assert_eq!(decide(0.5, 0.5, 0.5), KeepVerdict::KEEP);
    }

    #[tokio::test]
    async fn o_keep_all_nunca_omite() {
        let verdicts = KeepAllJudge.judge(&GoalContext::default(), &[]).await;
        assert!(verdicts.is_empty());
    }
}
