//! Per-agent spend ceiling, cut **before** the turn is sent. Owned by
//! f2-llm-coordinator.
//!
//! State lives in `<app_data>/llm/budget.json` (`OMNIGET_DATA_DIR` overrides the
//! root, like `core::secrets` and the profile store), keyed by agent id inside a
//! UTC day window. Everything the check touches is in memory, so the check is a
//! map lookup and two comparisons; the file is only rewritten when a turn is
//! recorded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::agent::Budget;
use super::error::{LlmError, ERR_LLM_BUDGET};

/// One agent's spend inside the current UTC day.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentSpend {
    #[serde(default)]
    pub usd: f64,
    #[serde(default)]
    pub turns: u32,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

impl AgentSpend {
    pub fn tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// The whole file: one UTC day and the per-agent totals inside it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BudgetState {
    /// `YYYY-MM-DD` in UTC. A different day wipes `agents`.
    #[serde(default)]
    pub day: String,
    #[serde(default)]
    pub agents: HashMap<String, AgentSpend>,
}

impl BudgetState {
    /// Drop the totals when the UTC day turned over.
    fn roll(&mut self, day: &str) {
        if self.day != day {
            self.day = day.to_string();
            self.agents.clear();
        }
    }
}

pub fn utc_day(now: DateTime<Utc>) -> String {
    now.format("%Y-%m-%d").to_string()
}

/// Default location: `<app_data>/llm/budget.json`.
pub fn default_path() -> Option<PathBuf> {
    crate::core::paths::app_data_dir().map(|d| d.join("llm").join("budget.json"))
}

#[derive(Debug)]
pub struct BudgetStore {
    path: Option<PathBuf>,
    state: Mutex<BudgetState>,
}

impl Default for BudgetStore {
    fn default() -> Self {
        Self::memory()
    }
}

impl BudgetStore {
    /// Load from `<app_data>/llm/budget.json`, or start empty if it is missing
    /// or corrupt (a broken ledger must never block the app).
    pub fn open() -> Self {
        match default_path() {
            Some(p) => Self::at(p),
            None => Self::memory(),
        }
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<BudgetState>(&t).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            state: Mutex::new(state),
        }
    }

    /// No file at all: tests and the `--no-persist` path.
    pub fn memory() -> Self {
        Self {
            path: None,
            state: Mutex::new(BudgetState::default()),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn snapshot(&self) -> BudgetState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn spent_today(&self, agent_id: &str) -> AgentSpend {
        self.spent_today_at(agent_id, Utc::now())
    }

    pub fn spent_today_at(&self, agent_id: &str, now: DateTime<Utc>) -> AgentSpend {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.roll(&utc_day(now));
        state.agents.get(agent_id).cloned().unwrap_or_default()
    }

    /// Dollars left today, `None` when the agent has no daily ceiling.
    pub fn remaining_usd(&self, agent_id: &str, budget: &Budget) -> Option<f64> {
        let cap = budget.usd_per_day?;
        Some((cap - self.spent_today(agent_id).usd).max(0.0))
    }

    /// The gate: called before every turn. Returns `ERR_LLM_BUDGET` when the
    /// daily ceiling is already reached or when the turn's own token estimate
    /// is over `tokens_per_turn`.
    pub fn check(
        &self,
        agent_id: &str,
        budget: &Budget,
        estimated_tokens: u32,
    ) -> Result<(), LlmError> {
        self.check_at(agent_id, budget, estimated_tokens, Utc::now())
    }

    pub fn check_at(
        &self,
        agent_id: &str,
        budget: &Budget,
        estimated_tokens: u32,
        now: DateTime<Utc>,
    ) -> Result<(), LlmError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.roll(&utc_day(now));
        if let Some(cap) = budget.usd_per_day {
            let spent = state.agents.get(agent_id).map(|s| s.usd).unwrap_or(0.0);
            if spent >= cap {
                return Err(LlmError::new(
                    ERR_LLM_BUDGET,
                    format!("daily budget reached: {spent:.4} of {cap:.4} USD"),
                ));
            }
        }
        if let Some(max) = budget.tokens_per_turn {
            if estimated_tokens > max {
                return Err(LlmError::new(
                    ERR_LLM_BUDGET,
                    format!("turn needs {estimated_tokens} tokens, the ceiling is {max}"),
                ));
            }
        }
        Ok(())
    }

    /// Book a finished turn and persist. Cost is what the provider reported, or
    /// what the pricing table computed upstream; `None` books zero.
    pub fn record(&self, agent_id: &str, usd: Option<f64>, input_tokens: u32, output_tokens: u32) {
        self.record_at(agent_id, usd, input_tokens, output_tokens, Utc::now());
    }

    pub fn record_at(
        &self,
        agent_id: &str,
        usd: Option<f64>,
        input_tokens: u32,
        output_tokens: u32,
        now: DateTime<Utc>,
    ) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.roll(&utc_day(now));
            let entry = state.agents.entry(agent_id.to_string()).or_default();
            entry.usd += usd.unwrap_or(0.0);
            entry.turns += 1;
            entry.input_tokens += input_tokens as u64;
            entry.output_tokens += output_tokens as u64;
        }
        self.save();
    }

    /// Atomic write: temp file next to the target, then rename.
    pub fn save(&self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        let state = self.snapshot();
        let Ok(text) = serde_json::to_string_pretty(&state) else {
            return;
        };
        if let Some(dir) = path.parent() {
            if std::fs::create_dir_all(dir).is_err() {
                return;
            }
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn day(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap()
    }

    fn budget(usd: Option<f64>, tokens: Option<u32>) -> Budget {
        Budget {
            usd_per_day: usd,
            tokens_per_turn: tokens,
            max_tool_calls_per_turn: 4,
        }
    }

    #[test]
    fn an_empty_budget_never_cuts() {
        let store = BudgetStore::memory();
        assert!(store.check("a", &Budget::default(), 1_000_000).is_ok());
    }

    #[test]
    fn cuts_when_the_daily_ceiling_is_reached() {
        let store = BudgetStore::memory();
        let b = budget(Some(1.0), None);
        let now = day(2026, 9, 18);
        store.record_at("a", Some(0.60), 100, 100, now);
        assert!(store.check_at("a", &b, 0, now).is_ok());
        store.record_at("a", Some(0.50), 100, 100, now);
        let err = store.check_at("a", &b, 0, now).unwrap_err();
        assert_eq!(err.code, ERR_LLM_BUDGET);
        assert!(err.message.contains("1.0000"));
    }

    #[test]
    fn cuts_when_the_turn_is_bigger_than_the_token_ceiling() {
        let store = BudgetStore::memory();
        let b = budget(None, Some(8_000));
        assert!(store.check("a", &b, 7_999).is_ok());
        let err = store.check("a", &b, 8_001).unwrap_err();
        assert_eq!(err.code, ERR_LLM_BUDGET);
    }

    #[test]
    fn the_ceiling_is_per_agent() {
        let store = BudgetStore::memory();
        let b = budget(Some(0.10), None);
        let now = day(2026, 9, 18);
        store.record_at("a", Some(0.50), 0, 0, now);
        assert!(store.check_at("a", &b, 0, now).is_err());
        assert!(store.check_at("b", &b, 0, now).is_ok());
    }

    #[test]
    fn the_utc_day_turning_over_wipes_the_ledger() {
        let store = BudgetStore::memory();
        let b = budget(Some(1.0), None);
        store.record_at("a", Some(2.0), 0, 0, day(2026, 9, 18));
        assert!(store.check_at("a", &b, 0, day(2026, 9, 18)).is_err());
        assert!(store.check_at("a", &b, 0, day(2026, 9, 19)).is_ok());
        assert_eq!(store.spent_today_at("a", day(2026, 9, 19)).usd, 0.0);
    }

    #[test]
    fn record_accumulates_tokens_and_turns() {
        let store = BudgetStore::memory();
        let now = day(2026, 9, 18);
        store.record_at("a", Some(0.01), 100, 50, now);
        store.record_at("a", Some(0.02), 10, 5, now);
        let spend = store.spent_today_at("a", now);
        assert_eq!(spend.turns, 2);
        assert_eq!(spend.input_tokens, 110);
        assert_eq!(spend.output_tokens, 55);
        assert_eq!(spend.tokens(), 165);
        assert!((spend.usd - 0.03).abs() < 1e-9);
    }

    #[test]
    fn remaining_usd_never_goes_negative() {
        let store = BudgetStore::memory();
        store.record("a", Some(5.0), 0, 0);
        assert_eq!(
            store.remaining_usd("a", &budget(Some(1.0), None)),
            Some(0.0)
        );
        assert_eq!(store.remaining_usd("a", &budget(None, None)), None);
    }

    #[test]
    fn persists_and_reloads_from_disk() {
        let dir = std::env::temp_dir().join(format!("omniget-budget-{}", uuid::Uuid::new_v4()));
        let path = dir.join("llm").join("budget.json");
        let store = BudgetStore::at(&path);
        let now = Utc::now();
        store.record_at("a", Some(0.25), 10, 20, now);
        assert!(path.exists(), "record writes the file");
        let again = BudgetStore::at(&path);
        assert_eq!(again.spent_today_at("a", now).usd, 0.25);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_starts_empty_instead_of_blocking() {
        let dir = std::env::temp_dir().join(format!("omniget-budget-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("budget.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = BudgetStore::at(&path);
        assert!(store.check("a", &budget(Some(1.0), None), 10).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_check_costs_well_under_a_hundred_microseconds() {
        let store = BudgetStore::memory();
        let b = budget(Some(10.0), Some(100_000));
        store.record("a", Some(0.1), 10, 10);
        let start = std::time::Instant::now();
        let n = 10_000;
        for _ in 0..n {
            store.check("a", &b, 1_000).unwrap();
        }
        let per = start.elapsed() / n;
        assert!(
            per < std::time::Duration::from_micros(100),
            "budget check took {per:?} per call"
        );
    }
}
