//! Team coordination — multi-agent collaboration with role-based task assignment
//!
//! A Team is a group of agents working together under a configured strategy.
//! Member work is routed through the canonical Subagent dispatcher when one is attached.

pub mod agent_box;
pub mod manager_subagent;
pub mod strategy;

pub use agent_box::ArcAgentBox;

use echo_core::agent::Agent;
use echo_core::tokenizer::UsageSummary;
use futures::{StreamExt, future::BoxFuture};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use tracing::info;

use super::types::SubagentDefinition;
use super::usage::LlmUsageStats;

/// Canonical member execution adapter used by live Team dispatch.
pub type TeamDispatchFn = Arc<
    dyn Fn(String, String) -> BoxFuture<'static, Result<super::types::SubagentResult, String>>
        + Send
        + Sync,
>;

// ── Team Role ─────────────────────────────────────────────────────────────────

/// Role of a team member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamRole {
    /// The coordinating agent that assigns tasks.
    Leader,
    /// A subagent that executes tasks.
    Subagent,
    /// A reviewer that validates outputs.
    Reviewer,
}

// ── Team Config ───────────────────────────────────────────────────────────────

/// Configuration for a team.
#[derive(Debug, Clone)]
pub struct TeamConfig {
    /// Maximum concurrent teammates.
    pub max_concurrent: usize,
    /// Default timeout for teammate tasks (seconds). 0 = no timeout.
    pub default_timeout_secs: u64,
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 5,
            // Aligned with AgentConfig.subagent_timeout_secs (600s = 10 min),
            // the single source of truth for all subagent dispatch timeouts
            // (Sync/Fork/Teammate + team). Sprint 5 unified this last blind
            // spot: previously 300. 0 = no timeout (see AgentConfig).
            default_timeout_secs: 600,
        }
    }
}

// ── Team Member ───────────────────────────────────────────────────────────────

/// A member of a team.
pub struct TeamMember {
    /// Member name.
    pub name: String,
    /// Team role.
    pub role: TeamRole,
    /// Agent instance.
    pub agent: Arc<dyn Agent>,
    /// Definition.
    pub definition: SubagentDefinition,
}

// ── Team ───────────────────────────────────────────────────────────────────────

/// A team of agents working together.
pub struct Team {
    /// Unique team ID.
    pub id: String,
    /// Human-readable team name.
    pub name: String,
    /// Configuration.
    pub config: TeamConfig,
    /// Team members.
    members: HashMap<String, TeamMember>,
}

impl Team {
    /// Create a new team with a leader.
    ///
    /// # Parameters
    /// * `id` - Unique team identifier.
    /// * `name` - Human-readable team name.
    /// * `config` - Team configuration.
    pub fn new(id: impl Into<String>, name: impl Into<String>, config: TeamConfig) -> Self {
        let id = id.into();
        Self {
            id,
            name: name.into(),
            config,
            members: HashMap::new(),
        }
    }

    /// Add a member to the team.
    ///
    /// # Parameters
    /// * `name` - Member name (must be unique within the team).
    /// * `role` - Role of the member (leader, subagent, reviewer).
    /// * `agent` - Agent instance.
    /// * `definition` - Subagent definition.
    pub fn add_member(
        &mut self,
        name: &str,
        role: TeamRole,
        agent: Box<dyn Agent>,
        definition: SubagentDefinition,
    ) {
        info!(team = %self.name, member = %name, role = ?role, "Adding team member");
        self.members.insert(
            name.to_string(),
            TeamMember {
                name: name.to_string(),
                role,
                agent: Arc::new(agent),
                definition,
            },
        );
    }

    /// Get a member by name.
    ///
    /// # Parameters
    /// * `name` - Member name.
    ///
    /// # Returns
    /// Reference to the team member if found, `None` otherwise.
    pub fn get_member(&self, name: &str) -> Option<&TeamMember> {
        self.members.get(name)
    }

    /// List all member names.
    ///
    /// # Returns
    /// Vector of member names.
    pub fn member_names(&self) -> Vec<String> {
        self.members.keys().cloned().collect()
    }

    /// List subagent names.
    ///
    /// # Returns
    /// Vector of names of members with `TeamRole::Subagent` role.
    pub fn subagent_names(&self) -> Vec<String> {
        self.members
            .iter()
            .filter(|(_, m)| m.role == TeamRole::Subagent)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Number of members.
    ///
    /// # Returns
    /// Count of team members.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Check if the team has no members.
    ///
    /// # Returns
    /// `true` if the team has zero members.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Get all members (for iteration).
    ///
    /// # Returns
    /// Iterator over team member references.
    pub fn members(&self) -> impl Iterator<Item = &TeamMember> {
        self.members.values()
    }

    /// Get the leader's name.
    pub fn leader_name(&self) -> Option<&str> {
        self.members()
            .find(|m| matches!(m.role, TeamRole::Leader))
            .map(|m| m.name.as_str())
    }

    /// Get all subagents.
    pub fn subagents(&self) -> impl Iterator<Item = &TeamMember> {
        self.members()
            .filter(|m| matches!(m.role, TeamRole::Subagent))
    }

    /// Human-readable list of subagents and their descriptions.
    pub fn subagent_descriptions(&self) -> String {
        self.subagents()
            .map(|member| format!("- {}: {}", member.name, member.definition.description))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn usage_snapshot(&self) -> HashMap<String, UsageSummary> {
        self.members
            .iter()
            .map(|(name, member)| (name.clone(), member.agent.token_usage_summary()))
            .collect()
    }

    fn usage_since(&self, before: &HashMap<String, UsageSummary>) -> Option<LlmUsageStats> {
        let mut models = BTreeSet::new();
        let mut usage = LlmUsageStats::default();

        for (name, member) in &self.members {
            let after = member.agent.token_usage_summary();
            let previous = before.get(name).cloned().unwrap_or_default();
            let calls = after.request_count.saturating_sub(previous.request_count);
            if calls > 0 && !after.model_name.is_empty() {
                models.insert(after.model_name.clone());
            }
            usage.prompt_tokens = usage.prompt_tokens.saturating_add(
                after
                    .total_prompt_tokens
                    .saturating_sub(previous.total_prompt_tokens),
            );
            usage.completion_tokens = usage.completion_tokens.saturating_add(
                after
                    .total_completion_tokens
                    .saturating_sub(previous.total_completion_tokens),
            );
            usage.total_tokens = usage
                .total_tokens
                .saturating_add(after.total_tokens.saturating_sub(previous.total_tokens));
            usage.cached_prompt_tokens = usage.cached_prompt_tokens.saturating_add(
                after
                    .total_cached_prompt_tokens
                    .saturating_sub(previous.total_cached_prompt_tokens),
            );
            usage.cache_creation_prompt_tokens = usage.cache_creation_prompt_tokens.saturating_add(
                after
                    .total_cache_creation_prompt_tokens
                    .saturating_sub(previous.total_cache_creation_prompt_tokens),
            );
            usage.call_count = usage.call_count.saturating_add(calls);
        }

        if usage.call_count == 0 && usage.total_tokens == 0 {
            return None;
        }
        usage.model = models.into_iter().collect::<Vec<_>>().join(",");
        usage.usage_reported = usage.call_count > 0;
        Some(usage)
    }
}

// ── TeamAgent ──────────────────────────────────────────────────────────────

/// A high-level orchestrator that runs a team with a given strategy.
///
/// Wraps [`Team`] and [`manager_subagent::ManagerSubagentOrchestrator`] to provide a simple
/// `execute(task)` interface suitable for use as a subagent or standalone runner.
pub struct TeamAgent {
    pub team: Team,
    pub strategy: strategy::TeamStrategy,
    /// Sprint 11: stable run_id for keying checkpoints (NOT `Team.id` which
    /// regenerates per build via uuid). `None` → in-memory, no persistence.
    pub run_id: Option<String>,
    /// Sprint 11: optional state store for checkpoint/resume. `None` → degrade
    /// to in-memory single-pass execution (today's behavior, backward-compat).
    pub state_store: Option<std::sync::Arc<dyn crate::state::RuntimeStateStore>>,
    pub cancel: echo_core::agent::CancellationToken,
    member_dispatch: Option<TeamDispatchFn>,
    dispatch_usage: Arc<std::sync::Mutex<LlmUsageStats>>,
}

/// Output and aggregate LLM usage for one team execution.
pub struct TeamExecutionResult {
    pub output: String,
    pub usage: Option<LlmUsageStats>,
}

impl TeamAgent {
    /// Create a new TeamAgent with the given team and strategy.
    pub fn new(team: Team, strategy: strategy::TeamStrategy) -> Self {
        Self {
            team,
            strategy,
            run_id: None,
            state_store: None,
            cancel: echo_core::agent::CancellationToken::new(),
            member_dispatch: None,
            dispatch_usage: Arc::new(std::sync::Mutex::new(LlmUsageStats::default())),
        }
    }

    /// Run a task through the team using the configured strategy.
    /// Wraps all subagent calls in tokio::time::timeout using team config.
    pub async fn execute(&self, task: &str) -> Result<String, String> {
        self.execute_with_usage(task)
            .await
            .map(|result| result.output)
    }

    /// Run a task and aggregate the token usage of every participating member.
    pub async fn execute_with_usage(&self, task: &str) -> Result<TeamExecutionResult, String> {
        let before = self.usage_snapshot();
        let timeout_secs = self.team.config.default_timeout_secs;
        let output = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err("Team execution cancelled".to_string()),
            result = async {
                if timeout_secs == 0 {
                    self.execute_inner(task).await
                } else {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(timeout_secs),
                        self.execute_inner(task),
                    )
                    .await
                    .unwrap_or_else(|_| Err(format!(
                        "Team execution timed out after {timeout_secs}s"
                    )))
                }
            } => result?,
        };
        Ok(TeamExecutionResult {
            output,
            usage: self.usage_since(&before),
        })
    }

    fn usage_snapshot(&self) -> (HashMap<String, UsageSummary>, LlmUsageStats) {
        let dispatched = self
            .dispatch_usage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        (self.team.usage_snapshot(), dispatched)
    }

    fn usage_since(
        &self,
        before: &(HashMap<String, UsageSummary>, LlmUsageStats),
    ) -> Option<LlmUsageStats> {
        if self.member_dispatch.is_none() {
            return self.team.usage_since(&before.0);
        }
        let after = self
            .dispatch_usage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut usage = LlmUsageStats {
            model: after.model,
            prompt_tokens: after.prompt_tokens.saturating_sub(before.1.prompt_tokens),
            completion_tokens: after
                .completion_tokens
                .saturating_sub(before.1.completion_tokens),
            total_tokens: after.total_tokens.saturating_sub(before.1.total_tokens),
            cached_prompt_tokens: after
                .cached_prompt_tokens
                .saturating_sub(before.1.cached_prompt_tokens),
            cache_creation_prompt_tokens: after
                .cache_creation_prompt_tokens
                .saturating_sub(before.1.cache_creation_prompt_tokens),
            usage_reported: after.usage_reported,
            call_count: after.call_count.saturating_sub(before.1.call_count),
        };
        if usage.call_count == 0 && usage.total_tokens == 0 {
            return None;
        }
        if usage.model.is_empty() {
            usage.model = "unknown".to_string();
        }
        Some(usage)
    }

    async fn execute_inner(&self, task: &str) -> Result<String, String> {
        match &self.strategy {
            strategy::TeamStrategy::ManagerSubagent => {
                let orch = manager_subagent::ManagerSubagentOrchestrator::new();
                orch.run_with_dispatch(
                    &self.team,
                    task,
                    self.run_id.as_deref(),
                    self.state_store.as_deref(),
                    self.member_dispatch.as_ref(),
                )
                .await
            }
            strategy::TeamStrategy::Pipeline(agents) => {
                let mut current = task.to_string();
                for agent_name in agents {
                    current = self
                        .execute_member(agent_name, &current)
                        .await
                        .map_err(|error| {
                            format!("Pipeline subagent {agent_name} failed: {error}")
                        })?;
                }
                Ok(current)
            }
            strategy::TeamStrategy::Debate { judge, debaters } => {
                // Collect proposals from all debaters
                let mut proposals = Vec::new();
                for name in debaters {
                    let proposal = self
                        .execute_member(name, task)
                        .await
                        .map_err(|error| format!("Debater {name} failed: {error}"))?;
                    proposals.push((name.clone(), proposal));
                }
                // Judge selects the best
                if self.team.get_member(judge).is_some() {
                    let proposals_text: String = proposals
                        .iter()
                        .map(|(n, p)| format!("From {n}:\n{p}\n"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let judge_prompt = format!(
                        "You are a judge. Review these proposals for the task: {task}\n\n\
                         {proposals_text}\n\
                         Select the best proposal and explain why. Then provide the final answer."
                    );
                    self.execute_member(judge, &judge_prompt)
                        .await
                        .map_err(|error| format!("Judge failed: {error}"))
                } else {
                    Err("Judge not found in team".into())
                }
            }
            strategy::TeamStrategy::Swarm {
                reducer,
                batch_size: _,
            } => {
                // Swarm: each subagent processes the task independently, reducer merges
                // Use a semaphore to respect max_concurrent from TeamConfig
                let subagents = self.team.subagent_names();
                let max_conc = self.team.config.max_concurrent.max(1);
                let member_tasks = subagents.into_iter().map(|name| async move {
                    let result = self.execute_member(&name, task).await;
                    (name, result)
                });
                let mut findings = Vec::new();
                let outcomes = futures::stream::iter(member_tasks)
                    .buffer_unordered(max_conc)
                    .collect::<Vec<_>>()
                    .await;
                for (name, result) in outcomes {
                    findings.push((
                        name.clone(),
                        result.map_err(|error| format!("Swarm subagent {name} failed: {error}"))?,
                    ));
                }
                let findings_text: String = findings
                    .iter()
                    .map(|(n, o)| format!("From {n}:\n{o}\n"))
                    .collect::<Vec<_>>()
                    .join("\n");
                if self.team.get_member(reducer).is_some() {
                    self.execute_member(
                        reducer,
                        &format!(
                            "You are a reducer. Merge these findings for the task: {task}\n\n\
                             {findings_text}\n\
                             Produce a single consolidated answer."
                        ),
                    )
                    .await
                    .map_err(|error| format!("Reducer failed: {error}"))
                } else if let Some(first) = findings.into_iter().next() {
                    Ok(first.1)
                } else {
                    Err("No findings to reduce".into())
                }
            }
        }
    }

    async fn execute_member(&self, name: &str, task: &str) -> Result<String, String> {
        if let Some(dispatch) = &self.member_dispatch {
            let result = dispatch(name.to_string(), task.to_string()).await?;
            if result.outcome.status != super::types::SubagentStatus::Completed {
                return Err(format!(
                    "subagent '{name}' ended with status {:?}: {}",
                    result.outcome.status, result.output
                ));
            }
            return Ok(result.output);
        }
        let member = self
            .team
            .get_member(name)
            .ok_or_else(|| format!("Team member '{name}' not found"))?;
        member
            .agent
            .execute(task)
            .await
            .map_err(|error| error.to_string())
    }
}

// ── TeamAgentBuilder ─────────────────────────────────────────────────

/// Fluent builder for [`TeamAgent`].
///
/// # Example
///
/// ```rust,ignore
/// let team = TeamAgent::builder()
///     .name("code-review")
///     .manager("lead", lead_agent, lead_def)
///     .subagent("explorer", explore_agent, explore_def)
///     .subagent("tester", test_agent, test_def)
///     .strategy(strategy::TeamStrategy::ManagerSubagent)
///     .build();
/// ```
pub struct TeamAgentBuilder {
    name: String,
    members: Vec<(String, TeamRole, Box<dyn Agent>, SubagentDefinition)>,
    strategy: strategy::TeamStrategy,
    /// Override TeamConfig.default_timeout_secs (seconds). None = use the
    /// default (600, aligned with AgentConfig.subagent_timeout_secs).
    /// Callers that hold the unified config (e.g. AgentConfig.subagent_timeout_secs)
    /// thread it here so team timeouts aren't a separate hardcoded island.
    default_timeout_secs: Option<u64>,
    /// Sprint 11: stable run_id for checkpoint keying. None → in-memory.
    run_id: Option<String>,
    /// Sprint 11: optional state store for checkpoint/resume. None → degrade.
    state_store: Option<std::sync::Arc<dyn crate::state::RuntimeStateStore>>,
    config: TeamConfig,
    cancel: echo_core::agent::CancellationToken,
    member_dispatch: Option<TeamDispatchFn>,
}

impl Default for TeamAgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TeamAgentBuilder {
    pub fn new() -> Self {
        Self {
            name: "team".into(),
            members: Vec::new(),
            strategy: strategy::TeamStrategy::default(),
            default_timeout_secs: None,
            run_id: None,
            state_store: None,
            config: TeamConfig::default(),
            cancel: echo_core::agent::CancellationToken::new(),
            member_dispatch: None,
        }
    }

    /// Set the team name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Add a manager (Leader role).
    pub fn manager(
        mut self,
        name: &str,
        agent: Box<dyn Agent>,
        definition: SubagentDefinition,
    ) -> Self {
        self.members
            .push((name.into(), TeamRole::Leader, agent, definition));
        self
    }

    /// Add a subagent (Subagent role).
    pub fn subagent(
        mut self,
        name: &str,
        agent: Box<dyn Agent>,
        definition: SubagentDefinition,
    ) -> Self {
        self.members
            .push((name.into(), TeamRole::Subagent, agent, definition));
        self
    }

    /// Add a reviewer.
    pub fn reviewer(
        mut self,
        name: &str,
        agent: Box<dyn Agent>,
        definition: SubagentDefinition,
    ) -> Self {
        self.members
            .push((name.into(), TeamRole::Reviewer, agent, definition));
        self
    }

    /// Set the collaboration strategy.
    pub fn strategy(mut self, strategy: strategy::TeamStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Override the team-wide default timeout (seconds). 0 = no timeout.
    /// When unset, `TeamConfig::default()` applies (600s, aligned with
    /// `AgentConfig.subagent_timeout_secs`). Thread the unified config here
    /// so team timeouts read from the same source as Sync/Fork/Teammate.
    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.default_timeout_secs = Some(secs);
        self
    }

    pub fn config(mut self, config: TeamConfig) -> Self {
        self.config = config;
        self
    }

    pub fn cancel(mut self, cancel: echo_core::agent::CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Route all member work through a canonical Subagent dispatcher.
    pub fn member_dispatch(mut self, dispatch: TeamDispatchFn) -> Self {
        self.member_dispatch = Some(dispatch);
        self
    }

    /// Sprint 11: set the stable run_id used to key checkpoint nodes. `None`
    /// (default) → team runs in-memory with no persistence. The run_id should
    /// come from `ExternalRunContext.run_id` (stable across retries), NOT
    /// `Team.id` (regenerated per build).
    pub fn run_id(mut self, run_id: Option<String>) -> Self {
        self.run_id = run_id;
        self
    }

    /// Sprint 11: inject a `RuntimeStateStore` for checkpoint/resume. `None`
    /// (default) → degrade to in-memory single-pass. When both `run_id` and a
    /// store are set, `ManagerSubagentOrchestrator` reads prior `TaskNode`s at
    /// entry (skip Success, reset+rerun Running/Failed) and writes plan /
    /// per-subagent / synthesis checkpoint nodes.
    pub fn state_store(
        mut self,
        store: Option<std::sync::Arc<dyn crate::state::RuntimeStateStore>>,
    ) -> Self {
        self.state_store = store;
        self
    }

    /// Build the TeamAgent.
    pub fn build(self) -> TeamAgent {
        let mut team = Team::new(
            format!("team_{}", uuid::Uuid::new_v4()),
            &self.name,
            self.config,
        );
        // Apply the unified timeout override (from AgentConfig.subagent_timeout_secs)
        // if the caller supplied one; otherwise the TeamConfig::default() (600s) stands.
        if let Some(secs) = self.default_timeout_secs {
            team.config.default_timeout_secs = secs;
        }

        for (name, role, agent, def) in self.members {
            team.add_member(&name, role, agent, def);
        }

        let mut agent = TeamAgent::new(team, self.strategy);
        // Sprint 11: plumb run_id + state_store into the agent (passed through
        // to ManagerSubagentOrchestrator::run for checkpoint/resume).
        agent.run_id = self.run_id;
        agent.state_store = self.state_store;
        agent.cancel = self.cancel;
        if let Some(dispatch) = self.member_dispatch {
            let usage = Arc::clone(&agent.dispatch_usage);
            agent.member_dispatch = Some(Arc::new(move |name, task| {
                let future = dispatch(name, task);
                let usage = Arc::clone(&usage);
                Box::pin(async move {
                    let result = future.await?;
                    if let Some(member_usage) = &result.usage {
                        let mut total = usage
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if !member_usage.model.is_empty() {
                            total.model = member_usage.model.clone();
                        }
                        total.prompt_tokens = total
                            .prompt_tokens
                            .saturating_add(member_usage.prompt_tokens);
                        total.completion_tokens = total
                            .completion_tokens
                            .saturating_add(member_usage.completion_tokens);
                        total.total_tokens =
                            total.total_tokens.saturating_add(member_usage.total_tokens);
                        total.cached_prompt_tokens = total
                            .cached_prompt_tokens
                            .saturating_add(member_usage.cached_prompt_tokens);
                        total.cache_creation_prompt_tokens = total
                            .cache_creation_prompt_tokens
                            .saturating_add(member_usage.cache_creation_prompt_tokens);
                        total.call_count = total.call_count.saturating_add(member_usage.call_count);
                        total.usage_reported |= member_usage.usage_reported;
                    }
                    Ok(result)
                })
            }));
        }
        agent
    }
}

impl TeamAgent {
    /// Create a new builder.
    pub fn builder() -> TeamAgentBuilder {
        TeamAgentBuilder::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockAgent;
    use echo_core::agent::AgentEvent;
    use echo_core::error::Result as AgentResult;
    use futures::future::BoxFuture;
    use futures::stream::{self, BoxStream};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct UsageAgent {
        calls: AtomicU64,
    }

    impl UsageAgent {
        fn new() -> Self {
            Self {
                calls: AtomicU64::new(0),
            }
        }
    }

    impl Agent for UsageAgent {
        fn name(&self) -> &str {
            "usage-agent"
        }

        fn model_name(&self) -> &str {
            "usage-model"
        }

        fn system_prompt(&self) -> &str {
            ""
        }

        fn execute<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, AgentResult<String>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::Relaxed);
                Ok("done".to_string())
            })
        }

        fn execute_stream<'a>(
            &'a self,
            task: &'a str,
        ) -> BoxFuture<'a, AgentResult<BoxStream<'a, AgentResult<AgentEvent>>>> {
            Box::pin(async move {
                let output = self.execute(task).await?;
                Ok(Box::pin(stream::once(
                    async move { Ok(AgentEvent::FinalAnswer(output)) },
                )) as BoxStream<'a, AgentResult<AgentEvent>>)
            })
        }

        fn token_usage_summary(&self) -> UsageSummary {
            let calls = self.calls.load(Ordering::Relaxed);
            UsageSummary {
                model_name: self.model_name().to_string(),
                total_prompt_tokens: calls.saturating_mul(10),
                total_completion_tokens: calls.saturating_mul(5),
                total_tokens: calls.saturating_mul(15),
                request_count: calls,
                ..UsageSummary::default()
            }
        }
    }

    #[test]
    fn test_team_new() {
        let team = Team::new("t1", "Test Team", TeamConfig::default());
        assert_eq!(team.id, "t1");
        assert_eq!(team.name, "Test Team");
        assert!(team.is_empty());
    }

    #[test]
    fn test_team_config_default_timeout_aligned() {
        // Sprint 5: TeamConfig.default_timeout_secs now reads 600 (aligned
        // with AgentConfig.subagent_timeout_secs). Guards against regression
        // to the old hardcoded 300.
        let cfg = TeamConfig::default();
        assert_eq!(cfg.default_timeout_secs, 600);
    }

    #[test]
    fn test_team_agent_builder_timeout_override() {
        // Sprint 5: the builder threads the unified config into TeamConfig.
        // No override → default (600); override → applied.
        let def = SubagentDefinition::simple_sync("leader");
        let default_team = TeamAgent::builder()
            .manager("leader", Box::new(MockAgent::new("leader")), def.clone())
            .build();
        assert_eq!(default_team.team.config.default_timeout_secs, 600);

        let overridden = TeamAgent::builder()
            .manager("leader", Box::new(MockAgent::new("leader")), def)
            .timeout_secs(120)
            .build();
        assert_eq!(overridden.team.config.default_timeout_secs, 120);
    }

    #[tokio::test]
    async fn team_execution_aggregates_member_usage() -> Result<(), String> {
        let team = TeamAgent::builder()
            .subagent(
                "usage-agent",
                Box::new(UsageAgent::new()),
                SubagentDefinition::simple_sync("usage-agent"),
            )
            .strategy(strategy::TeamStrategy::Pipeline(vec![
                "usage-agent".to_string(),
            ]))
            .build();

        let result = team.execute_with_usage("analyze").await?;
        let usage = result
            .usage
            .ok_or_else(|| "team usage was not aggregated".to_string())?;
        assert_eq!(result.output, "done");
        assert_eq!(usage.model, "usage-model");
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
        assert_eq!(usage.call_count, 1);
        Ok(())
    }

    #[test]
    fn test_team_add_members() {
        let mut team = Team::new("t1", "Team", TeamConfig::default());

        team.add_member(
            "leader",
            TeamRole::Leader,
            Box::new(MockAgent::new("leader")),
            SubagentDefinition::simple_sync("leader"),
        );
        team.add_member(
            "subagent1",
            TeamRole::Subagent,
            Box::new(MockAgent::new("subagent1")),
            SubagentDefinition::simple_sync("subagent1"),
        );

        assert_eq!(team.len(), 2);
        assert_eq!(team.subagent_names(), vec!["subagent1"]);
    }

    #[test]
    fn test_team_get_member() {
        let mut team = Team::new("t1", "Team", TeamConfig::default());
        team.add_member(
            "w",
            TeamRole::Subagent,
            Box::new(MockAgent::new("w")),
            SubagentDefinition::simple_sync("w"),
        );

        assert!(team.get_member("w").is_some());
        assert!(team.get_member("missing").is_none());
    }
}
