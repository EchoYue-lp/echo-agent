//! Subagent core types — definitions, execution modes, and results

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use super::usage::LlmUsageStats;

// ── Execution Mode ────────────────────────────────────────────────────────────

/// How a subagent executes relative to its parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ExecutionMode {
    /// Sync/delegate: parent blocks until subagent returns.
    /// Maps to the existing `AgentDispatchTool` behavior.
    #[default]
    Sync,
    /// Fork: inherits parent context (system prompt, tools, history),
    /// runs independently with optional timeout.
    Fork,
    /// Teammate: parallel independent agent with message-passing coordination.
    Teammate,
    /// Sprint 11: multi-agent team dispatch. Routes through `dispatch_team`
    /// which builds a `TeamAgent` from the `TeamSpec` on the definition.
    /// Unlike `Teammate` (single async agent poll), `Team` runs the full
    /// ManagerWorker plan→fan-out→synthesize pipeline with optional
    /// checkpoint/resume (when a `RuntimeStateStore` is configured).
    Team,
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionMode::Sync => write!(f, "sync"),
            ExecutionMode::Fork => write!(f, "fork"),
            ExecutionMode::Teammate => write!(f, "teammate"),
            ExecutionMode::Team => write!(f, "team"),
        }
    }
}

/// Isolation boundary that was actually established for a dispatch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObservedIsolation {
    Primary,
    Context,
    Worktree,
    Workspace,
    Worker,
    PrimaryFallback,
    #[default]
    Unknown,
}

impl ObservedIsolation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Context => "context",
            Self::Worktree => "worktree",
            Self::Workspace => "workspace",
            Self::Worker => "worker",
            Self::PrimaryFallback => "primary-fallback",
            Self::Unknown => "unknown",
        }
    }
}

// ── Subagent Kind ─────────────────────────────────────────────────────────────

/// How the subagent definition is sourced.
#[derive(Debug, Clone, Default)]
pub enum SubagentKind {
    /// Hard-coded agent provided by the library or application.
    #[default]
    BuiltIn,
    /// Loaded from a `.md` definition file (similar to skills).
    Custom {
        /// Path to the definition file.
        path: PathBuf,
    },
    /// Loaded from a plugin or remote registry.
    Plugin {
        /// Source identifier (e.g., plugin name or registry URL).
        source: String,
    },
}

// ── Team Spec (Sprint 11) ─────────────────────────────────────────────────────

/// Specification for a team-mode subagent (Sprint 11).
///
/// Carried on [`SubagentDefinition::team`]. The manager + workers are
/// referenced **by name** (late binding) — `dispatch_team` resolves them from
/// the `SubagentRegistry` at dispatch time. This decouples team topology from
/// instance lifetimes: each member is itself a normal registered subagent
/// (D-11-team-2: name-based late binding).
///
/// Only `TeamStrategy::ManagerWorker` is frontmatter-declarable (it's a unit
/// variant); `Pipeline`/`Debate`/`Swarm` carry inline agent-name data and are
/// programmatic-only (they remain without production callers — see spec §三
/// "范围外").
#[derive(Debug, Clone)]
pub struct TeamSpec {
    /// Strategy (typically `ManagerWorker`; others are programmatic-only).
    pub strategy: super::team::strategy::TeamStrategy,
    /// Manager/leader subagent name (must be separately registered).
    pub manager: String,
    /// Worker subagent names (must each be separately registered).
    pub workers: Vec<String>,
    /// Team runtime config (concurrency, timeout, etc.). Reuses `TeamConfig`.
    pub config: super::team::TeamConfig,
}

// ── Subagent Definition ───────────────────────────────────────────────────────

/// Declarative specification of a subagent.
///
/// Describes *what* the subagent looks like (model, tools, prompt, constraints)
/// without prescribing *how* to build it — that's the factory's job.
#[derive(Debug, Clone)]
pub struct SubagentDefinition {
    /// Unique name for discovery and dispatch.
    pub name: String,
    /// Human-readable description (exposed to the LLM in tool descriptions).
    pub description: String,
    /// How this agent is sourced.
    pub kind: SubagentKind,
    /// Default execution mode when dispatched.
    pub execution_mode: ExecutionMode,
    /// Model override (None = inherit from parent).
    pub model: Option<String>,
    /// System prompt override (None = inherit or auto-generate).
    pub system_prompt: Option<String>,
    /// Restrict available tools by name (None = inherit all from parent).
    pub tool_filter: Option<Vec<String>>,
    /// Max agent iterations (None = unlimited).
    pub max_iterations: Option<usize>,
    /// Token limit for this subagent (None = use default).
    pub token_limit: Option<usize>,
    /// How many recent messages to inherit from parent.
    /// `None` = don't inherit, `Some(0)` = inherit all, `Some(n)` = last n messages.
    pub inherit_history: Option<usize>,
    /// Whether to inherit parent memory store.
    pub inherit_memory: bool,
    /// Timeout in seconds (0 = no timeout).
    pub timeout_secs: u64,
    /// Whether this subagent can itself dispatch to further subagents.
    pub can_delegate: bool,
    /// Tags for discovery / filtering.
    pub tags: Vec<String>,
    /// Whether this sub-agent uses the lightweight (infrastructure-sharing) mode.
    ///
    /// When `true`, the sub-agent shares the parent's LLM client, ToolManager,
    /// and GuardManager instead of creating new instances.
    pub lightweight: bool,
    /// Whether Fork-dispatched execution of this subagent should run inside an
    /// isolated git worktree (Sprint 8). Mirrors Claude Code's
    /// `isolation: worktree` frontmatter. Only meaningful for **writer**
    /// workers (readonly workers don't mutate files and don't need isolation).
    ///
    /// When `true` AND a `WorktreeFactory` is configured on the executor, the
    /// Fork dispatch creates a worktree, binds it as the worker's `working_dir`,
    /// and finalizes a diff summary after the run. Worktree creation failure
    /// fails the dispatch (never silently continue without isolation). When
    /// `true` but no factory is configured, a warning is logged and the worker
    /// runs without isolation (the application decides whether to supply one).
    pub isolate_worktree: bool,
    /// Whether Fork-dispatched execution of this subagent should run inside an
    /// isolated data workspace (Sprint 10). For **data/research workers** that
    /// emit generated artifacts (CSVs/parquet/charts) — gives each worker a
    /// disjoint working directory so parallel runs don't overwrite each other's
    /// outputs, WITHOUT git coupling (unlike `isolate_worktree`, which suits
    /// code writers). When `true` AND a `DataWorkspaceFactory` is configured,
    /// the Fork dispatch creates a workspace (tmpdir), binds it as the worker's
    /// `working_dir`, and finalizes a file listing after the run. Workspace
    /// creation failure fails the dispatch. A worker should declare AT MOST ONE
    /// of `isolate_worktree` / `isolate_workspace` (worktree takes precedence if
    /// both are set, since a worktree also provides disjoint FS).
    pub isolate_workspace: bool,
    /// Sprint 11: team-mode specification. When `Some` AND
    /// `execution_mode == Team`, `dispatch_team` uses this to build the
    /// TeamAgent (resolving manager + workers by name from the registry).
    /// `None` for normal Sync/Fork/Teammate subagents.
    pub team: Option<TeamSpec>,
    /// When `true`, the role prefers background dispatch (Phase 2 schedules
    /// asynchronously). Phase 1 only stores the flag from frontmatter.
    pub is_background: bool,
}

impl SubagentDefinition {
    /// Create a minimal Sync-mode built-in definition.
    ///
    /// # Parameters
    /// * `name` - Unique name for discovery and dispatch.
    /// * `description` - Human-readable description exposed to the LLM.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind: SubagentKind::BuiltIn,
            execution_mode: ExecutionMode::Sync,
            model: None,
            system_prompt: None,
            tool_filter: None,
            max_iterations: None,
            token_limit: None,
            inherit_history: None,
            inherit_memory: false,
            timeout_secs: 0,
            can_delegate: false,
            tags: Vec::new(),
            lightweight: false,
            isolate_worktree: false,
            isolate_workspace: false,
            team: None,
            is_background: false,
        }
    }

    /// Convenience: a Sync-mode definition for backward-compatible registration.
    ///
    /// # Parameters
    /// * `name` - Unique name for discovery and dispatch.
    ///
    /// # Notes
    /// Auto-generates description as "Subagent {name}".
    pub fn simple_sync(name: impl Into<String>) -> Self {
        let name = name.into();
        Self::new(name.clone(), format!("Subagent {}", name))
    }
}

// ── Subagent Result ───────────────────────────────────────────────────────────

/// Default char budget for parent-facing summary when no `## Summary` heading.
const DEFAULT_SUMMARY_CHARS: usize = 1200;

/// Split raw subagent output into a parent-facing summary and artifact paths.
///
/// Looks for markdown headings `## Summary` and `## Artifacts`. If Summary is
/// missing, falls back to a UTF-8-safe truncation of the full text (never
/// panics on multi-byte characters).
pub fn split_subagent_output(raw: &str) -> (String, Vec<String>) {
    let summary = extract_markdown_section(raw, "Summary")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| raw.chars().take(DEFAULT_SUMMARY_CHARS).collect());
    let artifacts = extract_markdown_section(raw, "Artifacts")
        .map(|section| {
            section
                .lines()
                .filter_map(|line| {
                    let trimmed = line.trim();
                    let item = trimmed
                        .strip_prefix("- ")
                        .or_else(|| trimmed.strip_prefix("* "))
                        .unwrap_or(trimmed)
                        .trim();
                    if item.is_empty() {
                        None
                    } else {
                        Some(item.to_string())
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    (summary, artifacts)
}

fn extract_markdown_section<'a>(raw: &'a str, heading: &str) -> Option<&'a str> {
    let needle = format!("## {heading}");
    let start = raw.find(&needle)?;
    let after = raw.get(start.saturating_add(needle.len())..)?;
    let body_start = after.find('\n').map(|i| i.saturating_add(1)).unwrap_or(0);
    let body = after.get(body_start..)?;
    let end = body
        .find("\n## ")
        .or_else(|| body.find("\n# "))
        .unwrap_or_else(|| body.len());
    body.get(..end)
}

/// Result returned by a subagent execution.
#[derive(Debug, Clone)]
pub struct SubagentResult {
    /// Agent name that produced this result.
    pub agent_name: String,
    /// Final output text (full detail for UI / storage).
    pub output: String,
    /// Concise summary for the parent LLM. Filled by [`Self::with_structured`].
    pub summary: String,
    /// Artifact paths extracted from `## Artifacts` (may be empty).
    pub artifacts: Vec<String>,
    /// Execution duration.
    pub duration: Duration,
    /// Number of iterations used.
    pub iterations: usize,
    /// Token usage (if available).
    pub tokens_used: Option<usize>,
    /// Whether the output was truncated due to token limits.
    pub was_truncated: bool,
    /// Whether execution ended because its cancellation token fired.
    ///
    /// Cancellation is a terminal fact, not a successful textual result. The
    /// output remains populated for diagnostics, while callers use this flag
    /// to avoid marking the parent task completed.
    pub cancelled: bool,
    /// Execution mode that was used.
    pub mode: ExecutionMode,
    /// Isolation boundary actually established before model execution.
    pub isolation_observed: ObservedIsolation,
    /// Cumulative LLM usage across all calls in this dispatch.
    /// `None` when the agent produced no `LlmUsage` events (e.g. cancelled
    /// before first LLM call, or the provider never returned usage).
    pub usage: Option<LlmUsageStats>,
}

impl SubagentResult {
    /// Create a result for a synchronous (blocking) subagent execution.
    ///
    /// # Parameters
    /// * `agent_name` - Name of the agent that produced the result.
    /// * `output` - Final output text from the agent.
    /// * `duration` - Execution duration.
    pub fn sync_result(agent_name: &str, output: String, duration: Duration) -> Self {
        Self {
            agent_name: agent_name.to_string(),
            output,
            summary: String::new(),
            artifacts: Vec::new(),
            duration,
            iterations: 1,
            tokens_used: None,
            was_truncated: false,
            cancelled: false,
            mode: ExecutionMode::Sync,
            isolation_observed: ObservedIsolation::Unknown,
            usage: None,
        }
        .with_structured()
    }

    /// Create a result for a forked (non-blocking) subagent execution.
    ///
    /// # Parameters
    /// * `agent_name` - Name of the agent that produced the result.
    /// * `output` - Final output text from the agent.
    /// * `duration` - Execution duration.
    /// * `iterations` - Number of iterations used by the agent.
    pub fn fork_result(
        agent_name: &str,
        output: String,
        duration: Duration,
        iterations: usize,
    ) -> Self {
        Self {
            agent_name: agent_name.to_string(),
            output,
            summary: String::new(),
            artifacts: Vec::new(),
            duration,
            iterations,
            tokens_used: None,
            was_truncated: false,
            cancelled: false,
            mode: ExecutionMode::Fork,
            isolation_observed: ObservedIsolation::Unknown,
            usage: None,
        }
        .with_structured()
    }

    /// Fill [`Self::summary`] / [`Self::artifacts`] from [`Self::output`].
    pub fn with_structured(mut self) -> Self {
        let (summary, artifacts) = split_subagent_output(&self.output);
        self.summary = summary;
        self.artifacts = artifacts;
        self
    }
}

// ── Registered Subagent (view type) ──────────────────────────────────────────

/// A read-only snapshot of a registered subagent.
#[derive(Debug, Clone)]
pub struct RegisteredSubagent {
    /// Subagent definition.
    pub definition: SubagentDefinition,
    /// Whether the agent instance is currently available (factory or pre-built).
    pub has_instance: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_mode_default() {
        assert_eq!(ExecutionMode::default(), ExecutionMode::Sync);
    }

    #[test]
    fn test_execution_mode_display() {
        assert_eq!(ExecutionMode::Sync.to_string(), "sync");
        assert_eq!(ExecutionMode::Fork.to_string(), "fork");
        assert_eq!(ExecutionMode::Teammate.to_string(), "teammate");
        // Sprint 11: Team variant.
        assert_eq!(ExecutionMode::Team.to_string(), "team");
    }

    #[test]
    fn split_output_prefers_summary_heading() {
        let raw = "## Summary\n短结论\n\n## Artifacts\n- src/a.rs\n- docs/b.md\n\n## Notes\n细节";
        let (summary, artifacts) = split_subagent_output(raw);
        assert_eq!(summary, "短结论");
        assert_eq!(
            artifacts,
            vec!["src/a.rs".to_string(), "docs/b.md".to_string()]
        );
    }

    #[test]
    fn split_output_truncates_utf8_safely_without_heading() {
        let raw: String = "中文"
            .chars()
            .chain(std::iter::repeat('x').take(2000))
            .collect();
        let (summary, _) = split_subagent_output(&raw);
        assert_eq!(summary.chars().count(), DEFAULT_SUMMARY_CHARS);
        // Must not panic and must start with the multi-byte prefix.
        assert!(summary.starts_with("中文"));
    }

    #[test]
    fn test_team_spec_construction() {
        // Sprint 11: a TeamSpec with ManagerWorker strategy can be constructed
        // and attached to a SubagentDefinition. Workers are name-references.
        use super::super::team::TeamConfig;
        use super::super::team::strategy::TeamStrategy;
        let spec = TeamSpec {
            strategy: TeamStrategy::ManagerWorker,
            manager: "planner".to_string(),
            workers: vec!["explorer".to_string(), "summarizer".to_string()],
            config: TeamConfig::default(),
        };
        let mut def = SubagentDefinition::new("team-research", "team research worker");
        assert!(def.team.is_none());
        def.team = Some(spec.clone());
        assert_eq!(def.team.as_ref().unwrap().manager, "planner");
        assert_eq!(def.team.as_ref().unwrap().workers.len(), 2);
        assert_eq!(
            def.team.as_ref().unwrap().strategy,
            TeamStrategy::ManagerWorker
        );
    }

    #[test]
    fn test_subagent_definition_new() {
        let def = SubagentDefinition::new("researcher", "Researches topics");
        assert_eq!(def.name, "researcher");
        assert_eq!(def.execution_mode, ExecutionMode::Sync);
        assert!(matches!(def.kind, SubagentKind::BuiltIn));
        assert!(def.inherit_history.is_none());
    }

    #[test]
    fn test_simple_sync() {
        let def = SubagentDefinition::simple_sync("worker");
        assert_eq!(def.name, "worker");
        assert!(def.description.contains("worker"));
    }

    #[test]
    fn test_subagent_result_sync() {
        let result = SubagentResult::sync_result("a", "ok".into(), Duration::from_millis(100));
        assert_eq!(result.mode, ExecutionMode::Sync);
        assert_eq!(result.output, "ok");
    }
}
