//! Team coordination — multi-agent collaboration with role-based task assignment
//!
//! A Team is a group of agents working together under a coordinator.
//! The leader assigns tasks, teammates execute and report back via mailboxes.

pub mod coordinator;
pub mod mailbox;

use echo_core::agent::Agent;
use futures::lock::Mutex as AsyncMutex;
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
            default_timeout_secs: 300,
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
    pub agent: Arc<AsyncMutex<Box<dyn Agent>>>,
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
    /// # 参数
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
    /// # 参数
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
                agent: Arc::new(AsyncMutex::new(agent)),
                mailbox,
                definition,
            },
        );
    }

    /// Get a member by name.
    ///
    /// # 参数
    /// * `name` - Member name.
    ///
    /// # 返回
    /// Reference to the team member if found, `None` otherwise.
    pub fn get_member(&self, name: &str) -> Option<&TeamMember> {
        self.members.get(name)
    }

    /// Get a member's mailbox sender (for sending messages).
    ///
    /// # 参数
    /// * `name` - Member name.
    ///
    /// # 返回
    /// Mailbox sender for the member if found, `None` otherwise.
    pub fn get_mailbox_sender(&self, name: &str) -> Option<mailbox::MailboxSender> {
        self.members.get(name).map(|m| m.mailbox.sender())
    }

    /// List all member names.
    ///
    /// # 返回
    /// Vector of member names.
    pub fn member_names(&self) -> Vec<String> {
        self.members.keys().cloned().collect()
    }

    /// List worker names.
    ///
    /// # 返回
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
    /// # 返回
    /// Count of team members.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Check if the team has no members.
    ///
    /// # 返回
    /// `true` if the team has zero members.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Get all members (for iteration).
    ///
    /// # 返回
    /// Iterator over team member references.
    pub fn members(&self) -> impl Iterator<Item = &TeamMember> {
        self.members.values()
    }
}

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
