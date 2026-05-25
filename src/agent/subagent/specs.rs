//! Standard subagent specifications — predefined agent types with context policies.
//!
//! Each spec defines a complete agent profile: what it does, what tools it needs,
//! how much parent context it inherits, and sensible defaults for timeout and
//! execution mode.

use super::context::ContextInheritance;
use super::types::{ExecutionMode, SubagentDefinition, SubagentKind};

// ── ContextPolicy ────────────────────────────────────────────────────

/// Controls how much parent context a subagent receives.
///
/// This is the user-facing concept; it maps to [`ContextInheritance`] which
/// is the internal mechanism used by the executor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextPolicy {
    /// Inherit everything: system prompt, tools, recent history, memory.
    FullContext,
    /// Only the task description — no parent history or tools.
    TaskOnly,
    /// Only specific files (not yet implemented — falls back to TaskOnly).
    SelectedFiles(Vec<String>),
    /// Only git diff context — useful for review agents.
    DiffOnly,
    /// Only error messages — useful for fixer agents.
    ErrorOnly,
    /// Only a brief summary of the parent's conversation.
    SummaryOnly,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self::TaskOnly
    }
}

impl ContextPolicy {
    /// Convert this policy into a [`ContextInheritance`] configuration.
    pub fn to_inheritance(&self, mode: &ExecutionMode) -> ContextInheritance {
        match self {
            Self::FullContext => ContextInheritance::for_mode(mode),
            Self::TaskOnly => {
                let mut inh = ContextInheritance::for_mode(mode);
                inh.inherit_system_prompt = false;
                inh.inherit_history = None;
                inh.inherit_memory = false;
                inh
            }
            Self::SelectedFiles(files) => {
                // Inject specified file contents as metadata for the executor to expand
                let mut inh = ContextInheritance::for_mode(mode);
                inh.inherit_history = None;
                inh.inherit_memory = false;
                for f in files {
                    if let Ok(content) = std::fs::read_to_string(f) {
                        let preview: String = content.chars().take(2000).collect();
                        inh.inject_metadata.insert(format!("file:{f}"), preview);
                    }
                }
                inh
            }
            Self::DiffOnly => {
                let mut inh = ContextInheritance::for_mode(mode);
                inh.inherit_history = Some(1); // just the latest (likely the diff)
                inh.inherit_memory = false;
                inh
            }
            Self::ErrorOnly => {
                let mut inh = ContextInheritance::for_mode(mode);
                inh.inherit_history = Some(3); // last few messages with errors
                inh.inherit_memory = false;
                inh
            }
            Self::SummaryOnly => {
                let mut inh = ContextInheritance::for_mode(mode);
                inh.inherit_history = Some(2); // just enough for context
                inh.inherit_memory = false;
                inh
            }
        }
    }

    /// Human-readable description of this policy.
    pub fn description(&self) -> &str {
        match self {
            Self::FullContext => "Inherits all parent context (system prompt, tools, history, memory)",
            Self::TaskOnly => "Receives only the task description, no parent history",
            Self::SelectedFiles(_) => "Receives only specified files, no parent history",
            Self::DiffOnly => "Receives only git diff context",
            Self::ErrorOnly => "Receives only error messages from the parent",
            Self::SummaryOnly => "Receives a brief summary of parent conversation",
        }
    }
}

// ── SubAgentSpec ─────────────────────────────────────────────────────

/// A pre-configured subagent specification.
///
/// Specs can be instantiated into [`SubagentDefinition`]s and registered
/// with the [`SubagentRegistry`](super::registry::SubagentRegistry).
#[derive(Debug, Clone)]
pub struct SubAgentSpec {
    /// Unique name (e.g. "code-explorer").
    pub name: String,
    /// One-line description of what this agent does.
    pub role_description: String,
    /// System prompt template. `{task}` is replaced with the actual task.
    pub system_prompt_template: String,
    /// Tools this agent should have access to.
    pub recommended_tools: Vec<String>,
    /// How much parent context to inherit.
    pub context_policy: ContextPolicy,
    /// Default timeout in seconds.
    pub default_timeout_secs: u64,
    /// Preferred execution mode.
    pub execution_mode: ExecutionMode,
}

impl SubAgentSpec {
    /// Convert this spec into a [`SubagentDefinition`] ready for registration.
    pub fn to_definition(&self) -> SubagentDefinition {
        SubagentDefinition {
            name: self.name.clone(),
            description: self.role_description.clone(),
            kind: SubagentKind::BuiltIn,
            execution_mode: self.execution_mode.clone(),
            model: None, // inherit from parent
            system_prompt: Some(self.system_prompt_template.clone()),
            tool_filter: Some(self.recommended_tools.clone()),
            max_iterations: Some(10),
            token_limit: None,
            inherit_history: match self.context_policy {
                ContextPolicy::FullContext => Some(10),
                ContextPolicy::SummaryOnly => Some(2),
                ContextPolicy::DiffOnly => Some(1),
                ContextPolicy::ErrorOnly => Some(3),
                _ => None,
            },
            inherit_memory: matches!(self.context_policy, ContextPolicy::FullContext),
            timeout_secs: self.default_timeout_secs,
            can_delegate: false,
            tags: vec!["builtin".into(), self.name.clone()],
            lightweight: true, // share parent LLM client
        }
    }

    // ── Built-in specs ────────────────────────────────────────────

    /// A subagent specialized in exploring and understanding codebases.
    pub fn code_explorer() -> Self {
        Self {
            name: "code-explorer".into(),
            role_description: "Explores codebases: reads files, searches symbols, maps project structure".into(),
            system_prompt_template: concat!(
                "You are a Code Explorer agent. Your job is to understand code and answer questions about it.\n",
                "You can read files, search for patterns, and trace references.\n",
                "Be thorough: read related files, not just the one mentioned.\n",
                "Always report: what you found, which files are relevant, and how they connect.\n",
                "Do NOT edit files. Only read and analyze."
            ).into(),
            recommended_tools: vec![
                "read_file".into(), "shell".into(), "search".into(),
            ],
            context_policy: ContextPolicy::FullContext,
            default_timeout_secs: 120,
            execution_mode: ExecutionMode::Fork,
        }
    }

    /// A subagent specialized in running tests and parsing failures.
    pub fn test_runner() -> Self {
        Self {
            name: "test-runner".into(),
            role_description: "Runs tests, parses failures, reports which files need fixing".into(),
            system_prompt_template: concat!(
                "You are a Test Runner agent. Your job is to run tests and diagnose failures.\n",
                "Run the appropriate test command for the project.\n",
                "Parse the output to find which tests failed and why.\n",
                "For each failure, identify: the test name, the source file, and the error message.\n",
                "Report your findings clearly so another agent can fix the issues."
            ).into(),
            recommended_tools: vec![
                "shell".into(),
            ],
            context_policy: ContextPolicy::TaskOnly,
            default_timeout_secs: 300,
            execution_mode: ExecutionMode::Fork,
        }
    }

    /// A subagent specialized in security review.
    pub fn security_reviewer() -> Self {
        Self {
            name: "security-reviewer".into(),
            role_description: "Reviews code changes for security vulnerabilities".into(),
            system_prompt_template: concat!(
                "You are a Security Reviewer agent. Your job is to find security issues in code.\n",
                "Check for: injection attacks, XSS, insecure crypto, hardcoded secrets, ",
                "missing auth checks, path traversal, unsafe deserialization.\n",
                "For each finding, explain: the vulnerability, the risk level, and how to fix it.\n",
                "Do NOT edit files. Only report findings."
            ).into(),
            recommended_tools: vec![
                "read_file".into(), "shell".into(),
            ],
            context_policy: ContextPolicy::DiffOnly,
            default_timeout_secs: 180,
            execution_mode: ExecutionMode::Fork,
        }
    }

    /// A subagent specialized in fixing compilation/build errors.
    pub fn build_fixer() -> Self {
        Self {
            name: "build-fixer".into(),
            role_description: "Fixes compilation and build errors".into(),
            system_prompt_template: concat!(
                "You are a Build Fixer agent. Your job is to fix compilation errors.\n",
                "Read the error messages carefully and find the root cause.\n",
                "Make minimal changes to fix the issue — do not refactor or add features.\n",
                "After fixing, verify the build succeeds."
            ).into(),
            recommended_tools: vec![
                "read_file".into(), "edit_file".into(), "shell".into(),
            ],
            context_policy: ContextPolicy::ErrorOnly,
            default_timeout_secs: 300,
            execution_mode: ExecutionMode::Fork,
        }
    }

    /// A subagent specialized in writing documentation.
    pub fn doc_writer() -> Self {
        Self {
            name: "doc-writer".into(),
            role_description: "Writes and improves code documentation".into(),
            system_prompt_template: concat!(
                "You are a Documentation Writer agent. Your job is to create clear documentation.\n",
                "Write documentation that is: accurate, concise, well-structured.\n",
                "Include: function/method docs, module overviews, usage examples.\n",
                "Follow the project's existing documentation style."
            ).into(),
            recommended_tools: vec![
                "read_file".into(), "edit_file".into(), "write_file".into(),
            ],
            context_policy: ContextPolicy::FullContext,
            default_timeout_secs: 300,
            execution_mode: ExecutionMode::Fork,
        }
    }

    /// A subagent specialized in planning refactoring work.
    pub fn refactor_planner() -> Self {
        Self {
            name: "refactor-planner".into(),
            role_description: "Analyzes code and plans refactoring steps".into(),
            system_prompt_template: concat!(
                "You are a Refactor Planner agent. Your job is to plan code improvements.\n",
                "Analyze the target code and identify: duplication, coupling, naming issues, ",
                "missing abstractions, overly complex functions.\n",
                "Produce a step-by-step refactoring plan with risk assessment for each step.\n",
                "Do NOT make changes. Only plan."
            ).into(),
            recommended_tools: vec![
                "read_file".into(), "shell".into(),
            ],
            context_policy: ContextPolicy::FullContext,
            default_timeout_secs: 180,
            execution_mode: ExecutionMode::Fork,
        }
    }

    /// A subagent specialized in profiling and performance analysis.
    pub fn performance_profiler() -> Self {
        Self {
            name: "performance-profiler".into(),
            role_description: "Profiles code performance, identifies bottlenecks and hot paths".into(),
            system_prompt_template: concat!(
                "You are a Performance Profiler agent. Your job is to find performance issues.\n",
                "Look for: N+1 queries, unnecessary allocations, blocking I/O, ",
                "hot loops, excessive cloning, large memory footprints.\n",
                "For each finding, explain: the bottleneck, impact, and suggested fix.\n",
                "Do NOT make changes. Only profile and recommend."
            ).into(),
            recommended_tools: vec![
                "read_file".into(), "shell".into(),
            ],
            context_policy: ContextPolicy::FullContext,
            default_timeout_secs: 300,
            execution_mode: ExecutionMode::Fork,
        }
    }

    /// A subagent specialized in release engineering.
    pub fn release_engineer() -> Self {
        Self {
            name: "release-engineer".into(),
            role_description: "Manages version bumps, changelogs, and release notes".into(),
            system_prompt_template: concat!(
                "You are a Release Engineer agent. Your job is to prepare releases.\n",
                "Tasks: bump versions (semver), update changelogs, generate release notes.\n",
                "Check: are all tests passing? Is the changelog complete? ",
                "Are breaking changes documented?\n",
                "Produce a release checklist and suggested version number."
            ).into(),
            recommended_tools: vec![
                "read_file".into(), "edit_file".into(), "shell".into(), "git".into(),
            ],
            context_policy: ContextPolicy::FullContext,
            default_timeout_secs: 180,
            execution_mode: ExecutionMode::Fork,
        }
    }

    /// Return all built-in specs.
    pub fn all_builtin() -> Vec<Self> {
        vec![
            Self::code_explorer(),
            Self::test_runner(),
            Self::security_reviewer(),
            Self::build_fixer(),
            Self::doc_writer(),
            Self::refactor_planner(),
            Self::performance_profiler(),
            Self::release_engineer(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_policy_to_inheritance() {
        let inh = ContextPolicy::TaskOnly.to_inheritance(&ExecutionMode::Fork);
        assert!(!inh.inherit_system_prompt);
        assert!(inh.inherit_history.is_none());
        assert!(!inh.inherit_memory);

        let inh = ContextPolicy::FullContext.to_inheritance(&ExecutionMode::Fork);
        assert!(inh.inherit_system_prompt);
    }

    #[test]
    fn test_spec_to_definition() {
        let spec = SubAgentSpec::code_explorer();
        let def = spec.to_definition();
        assert_eq!(def.name, "code-explorer");
        assert!(def.tool_filter.is_some());
    }

    #[test]
    fn test_all_builtin() {
        let specs = SubAgentSpec::all_builtin();
        assert_eq!(specs.len(), 8);
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"code-explorer"));
        assert!(names.contains(&"test-runner"));
        assert!(names.contains(&"performance-profiler"));
        assert!(names.contains(&"release-engineer"));
    }
}
