//! Planning policy for framework-driven planning mode triggers
//!
//! This module defines the `PlanningPolicy` structure that uses rules to
//! determine when to trigger planning mode, rather than relying entirely on LLM judgment.

use super::plan_spec::Complexity;
use serde::{Deserialize, Serialize};

/// Planning policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningPolicy {
    /// Whether planning policy is enabled
    pub enabled: bool,

    /// Rules that trigger planning mode
    pub rules: Vec<PolicyRule>,

    /// Default execution mode when no rules match
    pub default_mode: ExecutionMode,
}

impl Default for PlanningPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            rules: vec![
                PolicyRule::MultipleArtifacts { threshold: 3 },
                PolicyRule::MultipleFiles { threshold: 3 },
                PolicyRule::MultiplePhases { threshold: 3 },
                PolicyRule::ManyToolCalls { threshold: 10 },
                PolicyRule::ParallelResearch,
                PolicyRule::LongRunningJob {
                    threshold_secs: 300,
                },
                PolicyRule::ContextPressure { threshold: 0.6 },
                PolicyRule::LlmComplexity {
                    threshold: Complexity::Medium,
                },
            ],
            default_mode: ExecutionMode::DirectExecute,
        }
    }
}

impl PlanningPolicy {
    /// Create a new planning policy
    pub fn new() -> Self {
        Self::default()
    }

    /// Determine whether to trigger planning mode based on context
    pub fn should_plan(&self, context: &PlanningContext) -> ExecutionMode {
        if !self.enabled {
            return ExecutionMode::DirectExecute;
        }

        for rule in &self.rules {
            if rule.matches(context) {
                return ExecutionMode::Plan {
                    reason: rule.describe(),
                };
            }
        }

        self.default_mode.clone()
    }

    /// Add a rule to the policy
    pub fn add_rule(&mut self, rule: PolicyRule) {
        self.rules.push(rule);
    }

    /// Remove all rules
    pub fn clear_rules(&mut self) {
        self.rules.clear();
    }
}

/// Policy rule that triggers planning mode
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PolicyRule {
    /// User goal contains multiple artifacts
    MultipleArtifacts { threshold: usize },

    /// Task requires modifying multiple files/modules
    MultipleFiles { threshold: usize },

    /// Task requires multiple phases (search/implement/verify)
    MultiplePhases { threshold: usize },

    /// Estimated tool calls exceed threshold
    ManyToolCalls { threshold: usize },

    /// Task requires parallel research
    ParallelResearch,

    /// Task requires long-running shell/job
    LongRunningJob { threshold_secs: u64 },

    /// Current context usage exceeds threshold
    ContextPressure { threshold: f32 },

    /// LLM self-assessed complexity
    LlmComplexity { threshold: Complexity },

    /// Historical failure rate for similar tasks
    HistoricalPattern { failure_rate: f32 },

    /// User preference override
    UserPreference { mode: ExecutionMode },
}

impl PolicyRule {
    /// Check if this rule matches the planning context
    pub fn matches(&self, context: &PlanningContext) -> bool {
        match self {
            PolicyRule::MultipleArtifacts { threshold } => {
                context.estimated_artifacts >= *threshold
            }
            PolicyRule::MultipleFiles { threshold } => context.estimated_files >= *threshold,
            PolicyRule::MultiplePhases { threshold } => context.estimated_phases >= *threshold,
            PolicyRule::ManyToolCalls { threshold } => context.estimated_tool_calls >= *threshold,
            PolicyRule::ParallelResearch => context.requires_parallel_research,
            PolicyRule::LongRunningJob { threshold_secs } => {
                context.estimated_duration_secs >= *threshold_secs
            }
            PolicyRule::ContextPressure { threshold } => {
                context.context_usage_percent >= *threshold
            }
            PolicyRule::LlmComplexity { threshold } => context.llm_complexity >= *threshold,
            PolicyRule::HistoricalPattern { failure_rate } => {
                context.historical_failure_rate >= *failure_rate
            }
            PolicyRule::UserPreference { mode } => context.user_preference.as_ref() == Some(mode),
        }
    }

    /// Describe why this rule triggered planning mode
    pub fn describe(&self) -> String {
        match self {
            PolicyRule::MultipleArtifacts { threshold } => {
                format!("Multiple artifacts detected (threshold: {})", threshold)
            }
            PolicyRule::MultipleFiles { threshold } => {
                format!(
                    "Multiple files require modification (threshold: {})",
                    threshold
                )
            }
            PolicyRule::MultiplePhases { threshold } => {
                format!(
                    "Multiple execution phases required (threshold: {})",
                    threshold
                )
            }
            PolicyRule::ManyToolCalls { threshold } => {
                format!(
                    "High number of tool calls expected (threshold: {})",
                    threshold
                )
            }
            PolicyRule::ParallelResearch => "Parallel research tasks detected".to_string(),
            PolicyRule::LongRunningJob { threshold_secs } => {
                format!("Long-running job expected (threshold: {}s)", threshold_secs)
            }
            PolicyRule::ContextPressure { threshold } => {
                format!(
                    "Context pressure high (threshold: {:.0}%)",
                    threshold * 100.0
                )
            }
            PolicyRule::LlmComplexity { threshold } => {
                format!("LLM assessed complexity as {:?}", threshold)
            }
            PolicyRule::HistoricalPattern { failure_rate } => {
                format!(
                    "Historical failure rate high ({:.0}%)",
                    failure_rate * 100.0
                )
            }
            PolicyRule::UserPreference { mode } => {
                format!("User preference: {:?}", mode)
            }
        }
    }
}

/// Execution mode decision
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Execute directly without planning
    DirectExecute,
    /// Trigger planning mode
    Plan { reason: String },
}

impl Default for ExecutionMode {
    fn default() -> Self {
        Self::DirectExecute
    }
}

/// Planning context for policy evaluation
#[derive(Debug, Clone)]
pub struct PlanningContext {
    /// User's stated goal
    pub user_goal: String,

    /// Estimated number of artifacts to produce
    pub estimated_artifacts: usize,

    /// Estimated number of files to modify
    pub estimated_files: usize,

    /// Estimated number of execution phases
    pub estimated_phases: usize,

    /// Estimated number of tool calls
    pub estimated_tool_calls: usize,

    /// Whether task requires parallel research
    pub requires_parallel_research: bool,

    /// Estimated duration in seconds
    pub estimated_duration_secs: u64,

    /// Current context usage as percentage (0.0 - 1.0)
    pub context_usage_percent: f32,

    /// LLM self-assessed complexity
    pub llm_complexity: Complexity,

    /// Historical failure rate for similar tasks (0.0 - 1.0)
    pub historical_failure_rate: f32,

    /// User preference override
    pub user_preference: Option<ExecutionMode>,
}

impl PlanningContext {
    /// Create a new planning context
    pub fn new(user_goal: impl Into<String>) -> Self {
        Self {
            user_goal: user_goal.into(),
            estimated_artifacts: 0,
            estimated_files: 0,
            estimated_phases: 0,
            estimated_tool_calls: 0,
            requires_parallel_research: false,
            estimated_duration_secs: 0,
            context_usage_percent: 0.0,
            llm_complexity: Complexity::Low,
            historical_failure_rate: 0.0,
            user_preference: None,
        }
    }

    /// Analyze user goal to estimate complexity
    pub fn analyze_goal(&mut self) {
        let goal = self.user_goal.to_lowercase();

        // Estimate artifacts based on keywords
        let artifact_keywords = ["report", "document", "file", "output", "result", "generate"];
        self.estimated_artifacts = artifact_keywords
            .iter()
            .filter(|kw| goal.contains(**kw))
            .count();

        // Estimate files based on keywords
        let file_keywords = ["modify", "edit", "update", "change", "refactor", "fix"];
        self.estimated_files = file_keywords
            .iter()
            .filter(|kw| goal.contains(**kw))
            .count();

        // Estimate phases based on keywords
        let phase_keywords = [
            "search",
            "research",
            "implement",
            "test",
            "verify",
            "deploy",
        ];
        self.estimated_phases = phase_keywords
            .iter()
            .filter(|kw| goal.contains(**kw))
            .count();

        // Detect parallel research
        let research_keywords = ["research", "investigate", "compare", "analyze"];
        self.requires_parallel_research = research_keywords
            .iter()
            .filter(|kw| goal.contains(**kw))
            .count()
            >= 1;

        // Estimate duration based on complexity
        self.estimated_duration_secs =
            (self.estimated_artifacts + self.estimated_files + self.estimated_phases) as u64 * 60; // 1 minute per unit

        // Estimate tool calls
        self.estimated_tool_calls =
            self.estimated_artifacts * 2 + self.estimated_files * 3 + self.estimated_phases * 5;

        // Estimate complexity
        let total_units = self.estimated_artifacts + self.estimated_files + self.estimated_phases;
        self.llm_complexity = if total_units >= 10 {
            Complexity::High
        } else if total_units >= 5 {
            Complexity::Medium
        } else {
            Complexity::Low
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let policy = PlanningPolicy::default();
        assert!(policy.enabled);
        assert!(!policy.rules.is_empty());
        assert_eq!(policy.default_mode, ExecutionMode::DirectExecute);
    }

    #[test]
    fn test_multiple_artifacts_rule() {
        let policy = PlanningPolicy::default();
        let mut context = PlanningContext::new("Generate multiple reports");
        context.estimated_artifacts = 5;

        let decision = policy.should_plan(&context);
        assert!(matches!(decision, ExecutionMode::Plan { .. }));
    }

    #[test]
    fn test_context_pressure_rule() {
        let policy = PlanningPolicy::default();
        let mut context = PlanningContext::new("Simple task");
        context.context_usage_percent = 0.8;

        let decision = policy.should_plan(&context);
        assert!(matches!(decision, ExecutionMode::Plan { .. }));
    }

    #[test]
    fn test_direct_execute_when_no_rules_match() {
        let policy = PlanningPolicy::default();
        let context = PlanningContext::new("Simple task");

        let decision = policy.should_plan(&context);
        assert_eq!(decision, ExecutionMode::DirectExecute);
    }

    #[test]
    fn test_disabled_policy() {
        let mut policy = PlanningPolicy::default();
        policy.enabled = false;

        let mut context = PlanningContext::new("Complex task");
        context.estimated_artifacts = 10;

        let decision = policy.should_plan(&context);
        assert_eq!(decision, ExecutionMode::DirectExecute);
    }

    #[test]
    fn test_analyze_goal() {
        let mut context =
            PlanningContext::new("Research and implement new feature, then test and verify");
        context.analyze_goal();

        assert!(context.estimated_phases >= 3); // research, implement, test, verify
        assert!(context.requires_parallel_research);
        assert!(context.estimated_tool_calls > 0);
    }

    #[test]
    fn test_complexity_ordering() {
        assert!(Complexity::Low < Complexity::Medium);
        assert!(Complexity::Medium < Complexity::High);
    }
}
