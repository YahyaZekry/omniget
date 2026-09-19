//! Team templates: ready-made agents the roster offers on the first run.
//! Owned by f2-llm-coordinator.
//!
//! The roles come from the multi-agent teams surveyed in `estudos/72` §2.5; the
//! prompts are written here, not copied. Every template routes instead of
//! pinning a model, so a user with only a local server still gets a working
//! team: the chain is filled in by the roster (`f2-llm-commands`) from the
//! keys and accounts that actually exist, and an empty chain simply means the
//! agent is not runnable yet.

use super::agent::{AgentDef, AgentRole, Budget, Candidate, ModelPolicy, RuntimeKind, ToolGrant};

/// A template before the roster binds it to real accounts.
#[derive(Debug, Clone, PartialEq)]
pub struct TeamTemplate {
    pub id: &'static str,
    /// i18n key for the display name; `name` is the English fallback.
    pub name_key: &'static str,
    pub name: &'static str,
    pub description_key: &'static str,
    pub role: AgentRole,
    pub system_prompt: &'static str,
    /// Suggested tool names (internal table); the roster turns them into grants.
    pub suggested_tools: &'static [&'static str],
    pub budget: Budget,
}

const COORDINATOR_PROMPT: &str = "\
You are the Chief of Staff of a small team of agents running inside OmniGet, a \
desktop app the user owns. You never do the work yourself when a specialist can \
do it better: you split the request into steps, name the agent for each step, \
and hand off with a written summary of what is already known and what is still \
open. State the plan in at most five lines before acting. When a step fails \
twice, stop and report instead of trying a third variation. You answer in the \
user's language.";

const ADVISOR_PROMPT: &str = "\
You are the Team Advisor. You are asked for judgement, not for execution: you \
review a plan, a diff or a decision and answer with what is wrong, what is \
missing and what you would do instead, in that order. You quote the specific \
line or claim you are objecting to. When you agree, you say so in one line \
instead of restating the plan. You never invent a fact to fill a gap; you name \
the gap.";

const ENGINEER_PROMPT: &str = "\
You are the Engineering Manager. You turn a request into a sequence of concrete \
changes: which file, which function, which command verifies it. You read before \
you write, you prefer editing an existing file over creating one, and you finish \
with the exact command that proves the change works. You never claim a \
measurement you did not run; if you did not measure it, you write that you did \
not.";

const NEXUS_PROMPT: &str = "\
You are the Nexus Coordinator. You watch what the machine is already doing — \
downloads, conversions, tool runs — and you keep the queue moving: you spot the \
job that is stuck, the file that landed in the wrong place, the retry that will \
never succeed. You act through tools, not through advice, and you report in one \
line per job: what it was, what you did, what the user has to decide.";

const CONFLICT_PROMPT: &str = "\
You are the Conflict Resolver. Two agents reached different conclusions and you \
decide. You restate each position in one sentence so both sides recognise \
themselves, identify the single fact or assumption they disagree on, and say \
which evidence would settle it. If the evidence is available, you get it and \
rule. If it is not, you say the question is open and name the cheapest \
experiment that would close it. You never split the difference to be polite.";

const DEFAULT_BUDGET: Budget = Budget {
    usd_per_day: Some(2.0),
    tokens_per_turn: Some(120_000),
    max_tool_calls_per_turn: 8,
};

const COORDINATOR_BUDGET: Budget = Budget {
    usd_per_day: Some(5.0),
    tokens_per_turn: Some(200_000),
    max_tool_calls_per_turn: 16,
};

pub const TEMPLATES: &[TeamTemplate] = &[
    TeamTemplate {
        id: "chief-of-staff",
        name_key: "llm.template.chief_of_staff.name",
        name: "Chief of Staff",
        description_key: "llm.template.chief_of_staff.desc",
        role: AgentRole::Coordinator,
        system_prompt: COORDINATOR_PROMPT,
        suggested_tools: &["dl_add", "dl_list", "tool_run"],
        budget: COORDINATOR_BUDGET,
    },
    TeamTemplate {
        id: "team-advisor",
        name_key: "llm.template.team_advisor.name",
        name: "Team Advisor",
        description_key: "llm.template.team_advisor.desc",
        role: AgentRole::Advisor,
        system_prompt: ADVISOR_PROMPT,
        suggested_tools: &[],
        budget: DEFAULT_BUDGET,
    },
    TeamTemplate {
        id: "engineering-manager",
        name_key: "llm.template.engineering_manager.name",
        name: "Engineering Manager",
        description_key: "llm.template.engineering_manager.desc",
        role: AgentRole::Worker,
        system_prompt: ENGINEER_PROMPT,
        suggested_tools: &["tool_run"],
        budget: DEFAULT_BUDGET,
    },
    TeamTemplate {
        id: "nexus-coordinator",
        name_key: "llm.template.nexus_coordinator.name",
        name: "Nexus Coordinator",
        description_key: "llm.template.nexus_coordinator.desc",
        role: AgentRole::Coordinator,
        system_prompt: NEXUS_PROMPT,
        suggested_tools: &["dl_add", "dl_list", "dl_retry", "tool_run"],
        budget: DEFAULT_BUDGET,
    },
    TeamTemplate {
        id: "conflict-resolver",
        name_key: "llm.template.conflict_resolver.name",
        name: "Conflict Resolver",
        description_key: "llm.template.conflict_resolver.desc",
        role: AgentRole::Advisor,
        system_prompt: CONFLICT_PROMPT,
        suggested_tools: &[],
        budget: DEFAULT_BUDGET,
    },
];

pub fn template(id: &str) -> Option<&'static TeamTemplate> {
    TEMPLATES.iter().find(|t| t.id == id)
}

impl TeamTemplate {
    /// Bind the template to a candidate chain and a grant list. An empty chain
    /// is legal: the agent exists in the roster and refuses to run until the
    /// user gives it a model.
    pub fn to_agent(&self, chain: Vec<Candidate>, tools: Vec<ToolGrant>) -> AgentDef {
        AgentDef {
            id: self.id.to_string(),
            name: self.name.to_string(),
            role: self.role.clone(),
            system_prompt: self.system_prompt.to_string(),
            model: ModelPolicy::Route { chain },
            tools,
            skills: Vec::new(),
            budget: self.budget.clone(),
            runtime: RuntimeKind::Native,
            skin: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_five_templates_with_unique_ids() {
        assert_eq!(TEMPLATES.len(), 5);
        let mut ids: Vec<&str> = TEMPLATES.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 5);
    }

    #[test]
    fn every_template_has_a_real_prompt_and_i18n_keys() {
        for t in TEMPLATES {
            assert!(t.system_prompt.len() > 200, "{} prompt is a stub", t.id);
            assert!(t.name_key.starts_with("llm.template."));
            assert!(t.description_key.starts_with("llm.template."));
            assert!(t.budget.max_tool_calls_per_turn > 0);
        }
    }

    #[test]
    fn lookup_by_id() {
        assert_eq!(
            template("conflict-resolver").unwrap().role,
            AgentRole::Advisor
        );
        assert!(template("nope").is_none());
    }

    #[test]
    fn to_agent_routes_instead_of_pinning() {
        let agent = template("chief-of-staff").unwrap().to_agent(vec![], vec![]);
        assert!(matches!(agent.model, ModelPolicy::Route { ref chain } if chain.is_empty()));
        assert_eq!(agent.runtime, RuntimeKind::Native);
        assert_eq!(agent.id, "chief-of-staff");
    }
}
