//! Team coordination — multi-agent collaboration with role-based task assignment
//!
//! A Team is a group of agents working together under a coordinator.
//! The leader assigns tasks, teammates execute and report back via mailboxes.

pub mod coordinator;
pub mod mailbox;
pub mod manager_worker;
pub mod message;
pub mod runner;
pub mod strategy;

pub use message::TeamMessage;
pub use runner::TeamRunner;

use echo_core::agent::Agent;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

use super::types::SubagentDefinition;
use coordinator::TeamCoordinator;
use mailbox::Mailbox;

// ── Team Role ─────────────────────────────────────────────────────────────────

/// Role of a team member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamRole {
    /// The coordinating agent that assigns tasks.
    Leader,
    /// A worker that executes tasks.
    Worker,
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
    /// Whether the leader can reassign tasks on failure.
    pub allow_reassignment: bool,
    /// Whether teammates can communicate with each other.
    pub cross_talk: bool,
    /// Mailbox capacity per teammate.
    pub mailbox_capacity: usize,
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
            allow_reassignment: true,
            cross_talk: false,
            mailbox_capacity: 64,
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
    /// Member's mailbox.
    pub mailbox: Mailbox,
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
    /// Task coordinator.
    pub coordinator: TeamCoordinator,
}

impl Team {
    /// Create a new team with a leader.
    ///
    /// # Parameters
    /// * `id` - Unique team identifier.
    /// * `name` - Human-readable team name.
    /// * `leader_name` - Name of the leader agent (must be added later via `add_member`).
    /// * `config` - Team configuration.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        leader_name: &str,
        config: TeamConfig,
    ) -> Self {
        let id = id.into();
        let leader = leader_name.to_string();
        Self {
            id,
            name: name.into(),
            config,
            members: HashMap::new(),
            coordinator: TeamCoordinator::new(&leader),
        }
    }

    /// Add a member to the team.
    ///
    /// # Parameters
    /// * `name` - Member name (must be unique within the team).
    /// * `role` - Role of the member (leader, worker, reviewer).
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
        let mailbox = Mailbox::with_capacity(self.config.mailbox_capacity);
        self.members.insert(
            name.to_string(),
            TeamMember {
                name: name.to_string(),
                role,
                agent: Arc::new(agent),
                mailbox,
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

    /// Get a member's mailbox sender (for sending messages).
    ///
    /// # Parameters
    /// * `name` - Member name.
    ///
    /// # Returns
    /// Mailbox sender for the member if found, `None` otherwise.
    pub fn get_mailbox_sender(&self, name: &str) -> Option<mailbox::MailboxSender> {
        self.members.get(name).map(|m| m.mailbox.sender())
    }

    /// List all member names.
    ///
    /// # Returns
    /// Vector of member names.
    pub fn member_names(&self) -> Vec<String> {
        self.members.keys().cloned().collect()
    }

    /// List worker names.
    ///
    /// # Returns
    /// Vector of names of members with `TeamRole::Worker` role.
    pub fn worker_names(&self) -> Vec<String> {
        self.members
            .iter()
            .filter(|(_, m)| m.role == TeamRole::Worker)
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

    /// Get all workers.
    pub fn workers(&self) -> impl Iterator<Item = &TeamMember> {
        self.members()
            .filter(|m| matches!(m.role, TeamRole::Worker))
    }

    /// Human-readable list of workers and their descriptions.
    pub fn worker_descriptions(&self) -> String {
        self.workers()
            .map(|w| format!("- {}: {}", w.name, w.definition.description))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ── TeamAgent ──────────────────────────────────────────────────────────────

/// A high-level orchestrator that runs a team with a given strategy.
///
/// Wraps [`Team`] and [`ManagerWorkerOrchestrator`] to provide a simple
/// `execute(task)` interface suitable for use as a subagent or standalone runner.
pub struct TeamAgent {
    pub team: Team,
    pub strategy: strategy::TeamStrategy,
}

impl TeamAgent {
    /// Create a new TeamAgent with the given team and strategy.
    pub fn new(team: Team, strategy: strategy::TeamStrategy) -> Self {
        Self { team, strategy }
    }

    /// Run a task through the team using the configured strategy.
    /// Wraps all subagent calls in tokio::time::timeout using team config.
    pub async fn execute(&self, task: &str) -> Result<String, String> {
        let timeout = std::time::Duration::from_secs(self.team.config.default_timeout_secs.max(60));
        tokio::time::timeout(timeout, self.execute_inner(task))
            .await
            .unwrap_or_else(|_| Err(format!("Team execution timed out after {:?}", timeout)))
    }

    async fn execute_inner(&self, task: &str) -> Result<String, String> {
        match &self.strategy {
            strategy::TeamStrategy::ManagerWorker => {
                let orch = manager_worker::ManagerWorkerOrchestrator::new();
                orch.run(&self.team, task).await
            }
            strategy::TeamStrategy::Pipeline(agents) => {
                let mut current = task.to_string();
                for agent_name in agents {
                    if let Some(member) = self.team.get_member(agent_name) {
                        current = member
                            .agent
                            .execute(&current)
                            .await
                            .map_err(|e| format!("Pipeline agent {agent_name} failed: {e}"))?;
                    }
                }
                Ok(current)
            }
            strategy::TeamStrategy::Debate { judge, debaters } => {
                // Collect proposals from all debaters
                let mut proposals = Vec::new();
                for name in debaters {
                    if let Some(member) = self.team.get_member(name) {
                        let proposal = member
                            .agent
                            .execute(task)
                            .await
                            .map_err(|e| format!("Debater {name} failed: {e}"))?;
                        proposals.push((name.clone(), proposal));
                    }
                }
                // Judge selects the best
                if let Some(judge_member) = self.team.get_member(judge) {
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
                    judge_member
                        .agent
                        .execute(&judge_prompt)
                        .await
                        .map_err(|e| format!("Judge failed: {e}"))
                } else {
                    Err("Judge not found in team".into())
                }
            }
            strategy::TeamStrategy::Swarm {
                reducer,
                batch_size: _,
            } => {
                // Swarm: each worker processes the task independently, reducer merges
                // Use a semaphore to respect max_concurrent from TeamConfig
                let workers: Vec<&TeamMember> = self.team.workers().collect();
                let max_conc = self.team.config.max_concurrent.max(1);
                let sem = Arc::new(tokio::sync::Semaphore::new(max_conc));
                let mut handles = Vec::new();
                for worker in &workers {
                    let agent = Arc::clone(&worker.agent);
                    let task = task.to_string();
                    let name = worker.name.clone();
                    let sem = sem.clone();
                    handles.push(tokio::spawn(async move {
                        let _permit = sem.acquire().await;
                        let result = agent.execute(&task).await;
                        (name, result)
                    }));
                }
                let mut findings = Vec::new();
                for h in handles {
                    if let Ok((name, Ok(output))) = h.await {
                        findings.push((name, output));
                    }
                }
                let findings_text: String = findings
                    .iter()
                    .map(|(n, o)| format!("From {n}:\n{o}\n"))
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Some(reducer_member) = self.team.get_member(reducer) {
                    reducer_member
                        .agent
                        .execute(&format!(
                            "You are a reducer. Merge these findings for the task: {task}\n\n\
                             {findings_text}\n\
                             Produce a single consolidated answer."
                        ))
                        .await
                        .map_err(|e| format!("Reducer failed: {e}"))
                } else if let Some(first) = findings.into_iter().next() {
                    Ok(first.1)
                } else {
                    Err("No findings to reduce".into())
                }
            }
        }
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
///     .worker("explorer", explore_agent, explore_def)
///     .worker("tester", test_agent, test_def)
///     .strategy(strategy::TeamStrategy::ManagerWorker)
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

    /// Add a worker (Worker role).
    pub fn worker(
        mut self,
        name: &str,
        agent: Box<dyn Agent>,
        definition: SubagentDefinition,
    ) -> Self {
        self.members
            .push((name.into(), TeamRole::Worker, agent, definition));
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

    /// Build the TeamAgent.
    pub fn build(self) -> TeamAgent {
        let leader_name = self
            .members
            .iter()
            .find(|(_, r, _, _)| matches!(r, TeamRole::Leader))
            .map(|(n, _, _, _)| n.as_str())
            .unwrap_or("leader");

        let mut team = Team::new(
            format!("team_{}", uuid::Uuid::new_v4()),
            &self.name,
            leader_name,
            TeamConfig::default(),
        );
        // Apply the unified timeout override (from AgentConfig.subagent_timeout_secs)
        // if the caller supplied one; otherwise the TeamConfig::default() (600s) stands.
        if let Some(secs) = self.default_timeout_secs {
            team.config.default_timeout_secs = secs;
        }

        for (name, role, agent, def) in self.members {
            team.add_member(&name, role, agent, def);
        }

        TeamAgent::new(team, self.strategy)
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

    #[test]
    fn test_team_new() {
        let team = Team::new("t1", "Test Team", "leader", TeamConfig::default());
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
    fn test_team_runner_default_timeout_aligned() {
        // Sprint 5: TeamRunner.timeout_secs aligned 120 → 600.
        let runner = TeamRunner::new();
        assert_eq!(runner.timeout_secs, 600);
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

    #[test]
    fn test_team_add_members() {
        let mut team = Team::new("t1", "Team", "leader", TeamConfig::default());

        team.add_member(
            "leader",
            TeamRole::Leader,
            Box::new(MockAgent::new("leader")),
            SubagentDefinition::simple_sync("leader"),
        );
        team.add_member(
            "worker1",
            TeamRole::Worker,
            Box::new(MockAgent::new("worker1")),
            SubagentDefinition::simple_sync("worker1"),
        );

        assert_eq!(team.len(), 2);
        assert_eq!(team.worker_names(), vec!["worker1"]);
    }

    #[test]
    fn test_team_get_member() {
        let mut team = Team::new("t1", "Team", "leader", TeamConfig::default());
        team.add_member(
            "w",
            TeamRole::Worker,
            Box::new(MockAgent::new("w")),
            SubagentDefinition::simple_sync("w"),
        );

        assert!(team.get_member("w").is_some());
        assert!(team.get_member("missing").is_none());
    }

    #[tokio::test]
    async fn test_team_mailbox_sender() {
        let mut team = Team::new("t1", "Team", "leader", TeamConfig::default());
        team.add_member(
            "w",
            TeamRole::Worker,
            Box::new(MockAgent::new("w")),
            SubagentDefinition::simple_sync("w"),
        );

        let sender = team.get_mailbox_sender("w").unwrap();
        let result = sender
            .send(mailbox::MailboxMessage::new(
                "leader",
                "w",
                mailbox::MessageKind::Status {
                    message: "ok".into(),
                },
            ))
            .await;
        assert!(result.is_ok());
    }
}
