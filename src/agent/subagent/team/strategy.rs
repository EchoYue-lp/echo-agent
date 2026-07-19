//! Team orchestration strategies.
//!
//! Defines how a team of agents coordinates to accomplish a task.

/// How a team of agents collaborates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TeamStrategy {
    /// One manager agent decomposes the task and fans out sub-tasks to subagents.
    /// Subagents execute independently and report results back to the manager,
    /// who synthesizes the final answer.
    #[default]
    ManagerSubagent,
    /// Agents run in a fixed sequence: each agent's output becomes the next
    /// agent's input. The last agent produces the final result.
    /// The `Vec<String>` specifies agent names in execution order.
    Pipeline(Vec<String>),
    /// Multiple agents independently propose solutions to the same task.
    /// A designated judge agent (the first name) selects the best proposal.
    /// The remaining names are the debaters.
    Debate {
        /// Judge agent name.
        judge: String,
        /// Debater agent names.
        debaters: Vec<String>,
    },
    /// Work is split across agents by module/file, each agent inspects its
    /// portion independently, then a reducer merges the findings.
    Swarm {
        /// How many items each subagent handles at a time.
        batch_size: usize,
        /// Reducer agent name (merges findings).
        reducer: String,
    },
}

impl TeamStrategy {
    /// Human-readable name of this strategy.
    pub fn name(&self) -> &str {
        match self {
            Self::ManagerSubagent => "manager-subagent",
            Self::Pipeline(_) => "pipeline",
            Self::Debate { .. } => "debate",
            Self::Swarm { .. } => "swarm",
        }
    }

    /// Description of how this strategy works.
    pub fn description(&self) -> &str {
        match self {
            Self::ManagerSubagent => {
                "Manager decomposes task → subagents execute sub-tasks → manager synthesizes result"
            }
            Self::Pipeline(_) => {
                "Agents run in sequence: each output becomes the next agent's input"
            }
            Self::Debate { .. } => "Multiple agents propose solutions, judge selects the best one",
            Self::Swarm { .. } => "Work is split across agents, reducer merges findings",
        }
    }
}
