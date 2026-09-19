//! Roster persistence: the list of `AgentDef` the user edits in `/llm`.
//!
//! One JSON array in `<app_data>/llm/roster.json`, written atomically
//! (tmp + rename) and kept in a memory cache so the UI can list the roster
//! without touching the disk (budget: reads must stay under 1 ms).
//! Owned by f2-llm-commands.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use super::agent::{
    AgentDef, AgentRole, Budget, GrantMode, ModelPolicy, RuntimeKind, ToolGrant, ToolSource,
};
use super::types::{ModelRef, ProviderId};

/// Stable error codes the UI maps. They live here (and not in `error.rs`,
/// owned by f2-llm-providers) so this module owns its own contract.
pub const ERR_ROSTER_DUP: &str = "ERR_LLM_ROSTER_DUP";
pub const ERR_ROSTER_MISSING: &str = "ERR_LLM_ROSTER_MISSING";
pub const ERR_ROSTER_IO: &str = "ERR_LLM_ROSTER_IO";
pub const ERR_ROSTER_ID: &str = "ERR_LLM_ROSTER_ID";
pub const ERR_ROSTER_TEMPLATE: &str = "ERR_LLM_ROSTER_TEMPLATE";

/// Error carrying a stable code, in the shape the Tauri layer turns into a
/// plain string for the frontend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterError {
    pub code: &'static str,
    pub message: String,
}

impl RosterError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RosterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RosterError {}

pub type Result<T> = std::result::Result<T, RosterError>;

/// Default folder for everything the LLM section persists.
pub fn llm_dir() -> Option<PathBuf> {
    crate::core::paths::app_data_dir().map(|d| d.join("llm"))
}

pub fn default_path() -> Option<PathBuf> {
    llm_dir().map(|d| d.join("roster.json"))
}

/// An agent id is a slug: lowercase letters, digits, `-` and `_`, 1..=64.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// The roster a fresh install starts with: one native assistant, no tools
/// granted. Nothing here calls the network.
/// The coding tools with the approved defaults: reading is automatic, anything
/// that writes or runs asks first.
pub fn code_grants() -> Vec<ToolGrant> {
    use super::code_tools::{READ_TOOLS, WRITE_TOOLS};
    let grant = |name: &str, mode| ToolGrant {
        source: ToolSource::Internal {
            name: name.to_string(),
        },
        mode,
    };
    READ_TOOLS
        .iter()
        .map(|n| grant(n, GrantMode::Auto))
        .chain(WRITE_TOOLS.iter().map(|n| grant(n, GrantMode::Ask)))
        .chain(std::iter::once(grant("agent_delegate", GrantMode::Ask)))
        .chain(std::iter::once(grant("kb_search", GrantMode::Auto)))
        .chain(std::iter::once(grant("kb_write", GrantMode::Ask)))
        .collect()
}

/// An agent that predates the coding tools gets them once, with the defaults.
fn with_code_grants(mut agents: Vec<AgentDef>) -> Vec<AgentDef> {
    for a in agents.iter_mut().filter(|a| a.id == "omni") {
        let has = a.tools.iter().any(|g| {
            matches!(&g.source, ToolSource::Internal { name } if name.starts_with("fs_") || name == "shell_exec")
        });
        if !has {
            a.tools.extend(code_grants());
        }
        for (tool, mode) in [("kb_search", GrantMode::Auto), ("kb_write", GrantMode::Ask)] {
            let has = a
                .tools
                .iter()
                .any(|g| matches!(&g.source, ToolSource::Internal { name } if name == tool));
            if !has {
                a.tools.push(ToolGrant {
                    source: ToolSource::Internal {
                        name: tool.to_string(),
                    },
                    mode,
                });
            }
        }
        let delegates = a.tools.iter().any(
            |g| matches!(&g.source, ToolSource::Internal { name } if name == "agent_delegate"),
        );
        if !delegates {
            a.tools.push(ToolGrant {
                source: ToolSource::Internal {
                    name: "agent_delegate".to_string(),
                },
                mode: GrantMode::Ask,
            });
        }
    }
    agents
}

fn candidate(provider: &str, model: &str) -> super::agent::Candidate {
    super::agent::Candidate {
        runtime: super::agent::CandidateRuntime::Native {
            provider: ProviderId::new(provider),
        },
        model: model.to_string(),
        max_cost_per_1k: None,
        min_context: 0,
    }
}

pub fn default_roster() -> Vec<AgentDef> {
    // Local first, so a fresh install works with nothing but Ollama; the
    // router walks down the chain when a candidate has no key or no quota.
    let local_first = || ModelPolicy::Route {
        chain: vec![
            candidate("ollama", "qwen3:8b"),
            candidate("anthropic", "claude-sonnet-5"),
            candidate("openai", "gpt-4o-mini"),
        ],
    };
    let agent =
        |id: &str, name: &str, role: AgentRole, prompt: &str, tools: Vec<ToolGrant>| AgentDef {
            id: id.into(),
            name: name.into(),
            role,
            system_prompt: prompt.into(),
            model: local_first(),
            tools,
            skills: Vec::new(),
            budget: Budget::default(),
            runtime: RuntimeKind::Native,
            skin: None,
        };
    // The scout reads and searches; it never writes and never runs a command.
    let read_only: Vec<ToolGrant> = code_grants()
        .into_iter()
        .filter(|g| g.mode == GrantMode::Auto)
        .collect();
    // Three residents, so the house is a team from the first visit. All of
    // them are ordinary roster entries: the user edits or deletes them.
    vec![
        agent(
            "omni",
            "Omni",
            AgentRole::Coordinator,
            "You are Omni, the assistant inside OmniGet. Answer briefly and \
             in the language the user writes in. Use a tool only when it is \
             clearly needed.",
            code_grants(),
        ),
        agent(
            "builder",
            "Builder",
            AgentRole::Worker,
            "You are Builder, the coding agent inside OmniGet. Read the code before \
             you change it, make the smallest edit that solves the task, and say \
             what you changed in one short paragraph.",
            code_grants(),
        ),
        agent(
            "scout",
            "Scout",
            AgentRole::Worker,
            "You are Scout, the reading and research agent inside OmniGet. Find the \
             relevant files and facts, quote where each one came from, and never \
             edit anything.",
            read_only,
        ),
    ]
}

/// Team templates. Placeholder table until `core/llm/templates.rs`
/// (f2-llm-coordinator) lands; `apply_template` delegates there once it exists.
pub fn template_ids() -> Vec<&'static str> {
    vec!["solo", "chief_of_staff", "engineering_manager"]
}

pub fn template(name: &str) -> Option<Vec<AgentDef>> {
    let base = |id: &str, agent_name: &str, role: AgentRole, prompt: &str| AgentDef {
        id: id.into(),
        name: agent_name.into(),
        role,
        system_prompt: prompt.into(),
        model: ModelPolicy::Fixed {
            model: ModelRef {
                provider: ProviderId::new("openai"),
                model: "gpt-4o-mini".into(),
            },
        },
        tools: Vec::new(),
        skills: Vec::new(),
        budget: Budget::default(),
        runtime: RuntimeKind::Native,
        skin: None,
    };
    match name {
        "solo" => Some(default_roster().into_iter().take(1).collect()),
        "chief_of_staff" => Some(vec![
            base(
                "chief",
                "Chief of Staff",
                AgentRole::Coordinator,
                "You run the user's day: you split a request into tasks, hand each one to \
                 a worker, and report back in one paragraph.",
            ),
            base(
                "researcher",
                "Researcher",
                AgentRole::Worker,
                "You gather facts and always say where each fact came from.",
            ),
            base(
                "writer",
                "Writer",
                AgentRole::Worker,
                "You turn notes into short, plain prose. No filler.",
            ),
        ]),
        "engineering_manager" => Some(vec![
            base(
                "em",
                "Engineering Manager",
                AgentRole::Coordinator,
                "You plan the work, decide the order and keep the scope honest.",
            ),
            base(
                "reviewer",
                "Reviewer",
                AgentRole::Advisor,
                "You review a change for correctness first and style last.",
            ),
        ]),
        _ => None,
    }
}

/// Cached, atomically written roster file.
pub struct RosterStore {
    path: PathBuf,
    cache: RwLock<Option<Arc<Vec<AgentDef>>>>,
}

impl RosterStore {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            cache: RwLock::new(None),
        }
    }

    /// Store at the default location, or `None` when there is no data dir.
    pub fn default_store() -> Option<Self> {
        default_path().map(Self::at)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Cached list. Reads disk once; every later call is a clone of the `Arc`.
    pub fn list(&self) -> Arc<Vec<AgentDef>> {
        if let Some(cached) = self.cache.read().ok().and_then(|g| g.clone()) {
            return cached;
        }
        let loaded = Arc::new(self.read_from_disk());
        if let Ok(mut g) = self.cache.write() {
            *g = Some(loaded.clone());
        }
        loaded
    }

    pub fn get(&self, id: &str) -> Option<AgentDef> {
        self.list().iter().find(|a| a.id == id).cloned()
    }

    pub fn create(&self, agent: AgentDef) -> Result<AgentDef> {
        if !valid_id(&agent.id) {
            return Err(RosterError::new(
                ERR_ROSTER_ID,
                format!("invalid agent id: {:?}", agent.id),
            ));
        }
        let mut all = (*self.list()).clone();
        if all.iter().any(|a| a.id == agent.id) {
            return Err(RosterError::new(
                ERR_ROSTER_DUP,
                format!("agent {} already exists", agent.id),
            ));
        }
        all.push(agent.clone());
        self.save(all)?;
        Ok(agent)
    }

    pub fn update(&self, agent: AgentDef) -> Result<AgentDef> {
        let mut all = (*self.list()).clone();
        let slot = all.iter_mut().find(|a| a.id == agent.id).ok_or_else(|| {
            RosterError::new(ERR_ROSTER_MISSING, format!("no agent {}", agent.id))
        })?;
        *slot = agent.clone();
        self.save(all)?;
        Ok(agent)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let mut all = (*self.list()).clone();
        let before = all.len();
        all.retain(|a| a.id != id);
        if all.len() == before {
            return Err(RosterError::new(
                ERR_ROSTER_MISSING,
                format!("no agent {id}"),
            ));
        }
        self.save(all)
    }

    /// Replaces the whole roster (used by `apply_template`).
    pub fn replace_all(&self, agents: Vec<AgentDef>) -> Result<Arc<Vec<AgentDef>>> {
        self.save(agents)?;
        Ok(self.list())
    }

    /// Applies a team template on top of the current roster: agents whose id
    /// already exists are left alone, so the call is idempotent.
    pub fn apply_template(&self, name: &str) -> Result<Arc<Vec<AgentDef>>> {
        let incoming = template(name).ok_or_else(|| {
            RosterError::new(ERR_ROSTER_TEMPLATE, format!("unknown template {name}"))
        })?;
        let mut all = (*self.list()).clone();
        for agent in incoming {
            if !all.iter().any(|a| a.id == agent.id) {
                all.push(agent);
            }
        }
        self.save(all)?;
        Ok(self.list())
    }

    fn read_from_disk(&self) -> Vec<AgentDef> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return default_roster();
        };
        match serde_json::from_str::<Vec<AgentDef>>(&text) {
            Ok(list) => with_code_grants(list),
            Err(error) => {
                tracing::warn!("[llm] roster.json is not readable ({error}); using the default");
                default_roster()
            }
        }
    }

    fn save(&self, agents: Vec<AgentDef>) -> Result<()> {
        let text = serde_json::to_string_pretty(&agents)
            .map_err(|e| RosterError::new(ERR_ROSTER_IO, e.to_string()))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| RosterError::new(ERR_ROSTER_IO, e.to_string()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text.as_bytes())
            .map_err(|e| RosterError::new(ERR_ROSTER_IO, e.to_string()))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| RosterError::new(ERR_ROSTER_IO, e.to_string()))?;
        if let Ok(mut g) = self.cache.write() {
            *g = Some(Arc::new(agents));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> RosterStore {
        let dir =
            std::env::temp_dir().join(format!("omniget-roster-{}-{}", tag, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        RosterStore::at(dir.join("roster.json"))
    }

    fn agent(id: &str) -> AgentDef {
        let mut a = default_roster().remove(0);
        a.id = id.into();
        a.name = id.to_uppercase();
        a
    }

    #[test]
    fn a_missing_file_yields_the_default_roster() {
        let store = temp_store("missing");
        let list = store.list();
        let ids: Vec<&str> = list.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["omni", "builder", "scout"]);
        assert!(!store.path().exists(), "listing must not create the file");
    }

    #[test]
    fn create_then_list_round_trips_through_disk() {
        let store = temp_store("create");
        store.create(agent("tester")).unwrap();
        let reread = RosterStore::at(store.path());
        let list = reread.list();
        assert_eq!(list.len(), 4);
        assert!(list.iter().any(|a| a.id == "tester"));
    }

    #[test]
    fn create_rejects_a_duplicate_id() {
        let store = temp_store("dup");
        store.create(agent("tester")).unwrap();
        let err = store.create(agent("tester")).unwrap_err();
        assert_eq!(err.code, ERR_ROSTER_DUP);
    }

    #[test]
    fn create_rejects_an_invalid_id() {
        let store = temp_store("badid");
        let err = store.create(agent("Not A Slug")).unwrap_err();
        assert_eq!(err.code, ERR_ROSTER_ID);
    }

    #[test]
    fn update_replaces_the_agent_and_fails_when_absent() {
        let store = temp_store("update");
        let mut a = agent("tester");
        store.create(a.clone()).unwrap();
        a.name = "Tester II".into();
        store.update(a).unwrap();
        assert_eq!(store.get("tester").unwrap().name, "Tester II");
        assert_eq!(
            store.update(agent("ghost")).unwrap_err().code,
            ERR_ROSTER_MISSING
        );
    }

    #[test]
    fn delete_removes_the_agent_and_fails_when_absent() {
        let store = temp_store("delete");
        store.create(agent("tester")).unwrap();
        store.delete("tester").unwrap();
        assert!(store.get("tester").is_none());
        assert_eq!(store.delete("tester").unwrap_err().code, ERR_ROSTER_MISSING);
    }

    #[test]
    fn apply_template_is_idempotent() {
        let store = temp_store("template");
        let first = store.apply_template("chief_of_staff").unwrap().len();
        let second = store.apply_template("chief_of_staff").unwrap().len();
        assert_eq!(first, second);
        assert_eq!(
            store.apply_template("nope").unwrap_err().code,
            ERR_ROSTER_TEMPLATE
        );
    }

    #[test]
    fn every_template_id_resolves() {
        for id in template_ids() {
            assert!(template(id).is_some(), "template {id} missing");
        }
    }

    #[test]
    fn valid_id_accepts_slugs_only() {
        assert!(valid_id("omni"));
        assert!(valid_id("team-2_a"));
        assert!(!valid_id(""));
        assert!(!valid_id("Omni"));
        assert!(!valid_id("omni agent"));
        assert!(!valid_id(&"x".repeat(65)));
    }

    #[test]
    fn a_corrupt_file_falls_back_to_the_default() {
        let store = temp_store("corrupt");
        std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        std::fs::write(store.path(), b"{ not json").unwrap();
        assert_eq!(store.list()[0].id, "omni");
    }

    #[test]
    fn the_cache_answers_without_touching_disk_again() {
        let store = temp_store("cache");
        store.create(agent("tester")).unwrap();
        std::fs::remove_file(store.path()).unwrap();
        assert_eq!(
            store.list().len(),
            4,
            "cache must survive the file going away"
        );
    }
}
