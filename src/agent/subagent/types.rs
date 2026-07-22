//! Subagent core types — definitions, execution modes, and results

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;
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
    /// Fork: runs independently with optional timeout. It starts with the
    /// registered role prompt and receives filtered history only when an
    /// explicit context-transfer policy requests it.
    Fork,
    /// Teammate: parallel independent agent with message-passing coordination.
    Teammate,
    /// Sprint 11: multi-agent team dispatch. Routes through `dispatch_team`
    /// which builds a `TeamAgent` from the `TeamSpec` on the definition.
    /// Unlike `Teammate` (single async agent poll), `Team` runs the full
    /// ManagerSubagent plan→fan-out→synthesize pipeline with optional
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
    Subagent,
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
            Self::Subagent => "subagent",
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
/// Carried on [`SubagentDefinition::team`]. The manager + subagents are
/// referenced **by name** (late binding) — `dispatch_team` resolves them from
/// the `SubagentRegistry` at dispatch time. This decouples team topology from
/// instance lifetimes: each member is itself a normal registered subagent
/// (D-11-team-2: name-based late binding).
///
/// Only `TeamStrategy::ManagerSubagent` is frontmatter-declarable (it's a unit
/// variant); `Pipeline`/`Debate`/`Swarm` carry inline agent-name data and are
/// programmatic-only (they remain without production callers — see spec §三
/// "范围外").
#[derive(Debug, Clone)]
pub struct TeamSpec {
    /// Strategy (typically `ManagerSubagent`; others are programmatic-only).
    pub strategy: super::team::strategy::TeamStrategy,
    /// Manager/leader subagent name (must be separately registered).
    pub manager: String,
    /// Team member subagent names (must each be separately registered).
    pub subagents: Vec<String>,
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
    /// subagents (readonly subagents don't mutate files and don't need isolation).
    ///
    /// When `true` AND a `WorktreeFactory` is configured on the executor, the
    /// Fork dispatch creates a worktree, binds it as the subagent's `working_dir`,
    /// and finalizes a diff summary after the run. Worktree creation failure
    /// fails the dispatch (never silently continue without isolation). When
    /// `true` but no factory is configured, a warning is logged and the subagent
    /// runs without isolation (the application decides whether to supply one).
    pub isolate_worktree: bool,
    /// Whether Fork-dispatched execution of this subagent should run inside an
    /// isolated data workspace (Sprint 10). For **data/research subagents** that
    /// emit generated artifacts (CSVs/parquet/charts) — gives each subagent a
    /// disjoint working directory so parallel runs don't overwrite each other's
    /// outputs, WITHOUT git coupling (unlike `isolate_worktree`, which suits
    /// code writers). When `true` AND a `DataWorkspaceFactory` is configured,
    /// the Fork dispatch creates a workspace (tmpdir), binds it as the subagent's
    /// `working_dir`, and finalizes a file listing after the run. Workspace
    /// creation failure fails the dispatch. A subagent should declare AT MOST ONE
    /// of `isolate_worktree` / `isolate_workspace` (worktree takes precedence if
    /// both are set, since a worktree also provides disjoint FS).
    pub isolate_workspace: bool,
    /// Sprint 11: team-mode specification. When `Some` AND
    /// `execution_mode == Team`, `dispatch_team` uses this to build the
    /// TeamAgent (resolving manager + subagents by name from the registry).
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

/// Default char budget for parent-facing summary when no structured result exists.
const DEFAULT_SUMMARY_CHARS: usize = 1200;
pub const SUBAGENT_RESULT_CONTRACT_VERSION: u32 = 1;
const MAX_RESULT_ITEMS: usize = 64;
const MAX_DETAIL_CHARS: usize = 500;
const MAX_PATH_CHARS: usize = 2048;
const MAX_KIND_CHARS: usize = 80;

/// Runtime-owned terminal status for one subagent dispatch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Completed,
    #[default]
    Failed,
    Cancelled,
    TimedOut,
}

impl SubagentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}

/// Availability and integrity facts for one artifact returned by a subagent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentArtifact {
    pub path: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub bytes: Option<u64>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub producer_execution_id: Option<String>,
    #[serde(default)]
    pub available: bool,
}

/// Result of one verification command or check.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentVerificationStatus {
    Passed,
    Failed,
    #[default]
    NotRun,
}

/// Whether verification evidence came from observed tool execution or subagent output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentVerificationSource {
    Observed,
    #[default]
    Reported,
}

/// Structured evidence for one verification check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentVerification {
    pub check: String,
    pub status: SubagentVerificationStatus,
    #[serde(default)]
    pub details: String,
    #[serde(default)]
    pub source: SubagentVerificationSource,
}

/// Files the subagent reports or the runtime observes reading and writing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentTouchedFiles {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub written: Vec<String>,
}

/// Parent-facing, serializable result contract for a subagent dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentOutcome {
    /// `1` for the explicit JSON contract; `0` for a legacy text fallback.
    pub contract_version: u32,
    /// Runtime-owned terminal status. Model-provided status is always ignored.
    pub status: SubagentStatus,
    pub summary: String,
    #[serde(default)]
    pub artifacts: Vec<SubagentArtifact>,
    #[serde(default)]
    pub verification: Vec<SubagentVerification>,
    #[serde(default)]
    pub remaining_work: Vec<String>,
    #[serde(default)]
    pub touched_files: SubagentTouchedFiles,
}

impl Default for SubagentOutcome {
    fn default() -> Self {
        Self {
            contract_version: 0,
            status: SubagentStatus::Failed,
            summary: String::new(),
            artifacts: Vec::new(),
            verification: Vec::new(),
            remaining_work: Vec::new(),
            touched_files: SubagentTouchedFiles::default(),
        }
    }
}

impl SubagentOutcome {
    pub fn terminal(
        status: SubagentStatus,
        summary: impl Into<String>,
        remaining_work: Vec<String>,
    ) -> Self {
        Self {
            contract_version: SUBAGENT_RESULT_CONTRACT_VERSION,
            status,
            summary: summary.into(),
            artifacts: Vec::new(),
            verification: Vec::new(),
            remaining_work,
            touched_files: SubagentTouchedFiles::default(),
        }
    }

    pub fn is_completed(&self) -> bool {
        self.status == SubagentStatus::Completed
    }
}

/// Render the canonical model-facing result contract parsed by
/// [`parse_subagent_outcome`]. Product layers may add optional sections before
/// this block, but the final `## Result` envelope remains framework-owned.
pub fn render_result_contract() -> String {
    format!(
        "## Result\nEnd the response with exactly one fenced JSON object using this shape:\n\
```json\n\
{{\"contract_version\":{SUBAGENT_RESULT_CONTRACT_VERSION},\"status\":\"completed\",\
\"summary\":\"at most 1200 characters\",\"artifacts\":[{{\"path\":\"actual path\",\
\"kind\":\"file|report|chart|other\"}}],\"verification\":[{{\"check\":\"exact command or check\",\
\"status\":\"passed\",\"details\":\"bounded evidence\",\
\"source\":\"reported\"}}],\"remaining_work\":[],\"touched_files\":{{\"read\":[],\
\"written\":[]}}}}\n\
```\n\
Verification status must be `passed`, `failed`, or `not_run`. Runtime owns terminal status and observed evidence. Report only real paths and checks; put incomplete or blocked work in remaining_work."
    )
}

#[derive(Debug, Deserialize)]
struct ReportedSubagentOutcome {
    #[serde(default)]
    contract_version: u32,
    #[allow(dead_code)]
    #[serde(default)]
    status: Option<SubagentStatus>,
    summary: String,
    #[serde(default)]
    artifacts: Vec<SubagentArtifact>,
    #[serde(default)]
    verification: Vec<SubagentVerification>,
    #[serde(default)]
    remaining_work: Vec<String>,
    #[serde(default)]
    touched_files: SubagentTouchedFiles,
}

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

/// Parse the explicit `## Result` JSON contract, falling back to a bounded summary.
///
/// The caller supplies the terminal status. A model cannot turn a failed, cancelled,
/// or timed-out execution into `completed` by returning a different JSON value.
pub fn parse_subagent_outcome(
    raw: &str,
    status: SubagentStatus,
    execution_id: Option<&str>,
    working_dir: Option<&Path>,
) -> SubagentOutcome {
    let reported = extract_markdown_section(raw, "Result")
        .and_then(extract_fenced_json)
        .and_then(|json| serde_json::from_str::<ReportedSubagentOutcome>(json).ok())
        .filter(|result| result.contract_version == SUBAGENT_RESULT_CONTRACT_VERSION);

    let mut outcome = if let Some(reported) = reported {
        SubagentOutcome {
            contract_version: SUBAGENT_RESULT_CONTRACT_VERSION,
            status,
            summary: reported
                .summary
                .trim()
                .chars()
                .take(DEFAULT_SUMMARY_CHARS)
                .collect(),
            artifacts: reported.artifacts,
            verification: reported
                .verification
                .into_iter()
                .map(|mut verification| {
                    // Only runtime-observed tool events may create observed evidence.
                    verification.source = SubagentVerificationSource::Reported;
                    verification
                })
                .collect(),
            remaining_work: bounded_unique(
                reported.remaining_work,
                MAX_RESULT_ITEMS,
                MAX_DETAIL_CHARS,
            ),
            touched_files: SubagentTouchedFiles {
                read: bounded_unique(
                    reported.touched_files.read,
                    MAX_RESULT_ITEMS,
                    MAX_PATH_CHARS,
                ),
                written: bounded_unique(
                    reported.touched_files.written,
                    MAX_RESULT_ITEMS,
                    MAX_PATH_CHARS,
                ),
            },
        }
    } else {
        let (summary, artifact_paths) = split_subagent_output(raw);
        SubagentOutcome {
            contract_version: 0,
            status,
            summary,
            artifacts: artifact_paths
                .into_iter()
                .map(|path| SubagentArtifact {
                    path,
                    kind: String::new(),
                    bytes: None,
                    sha256: None,
                    producer_execution_id: execution_id.map(str::to_string),
                    available: false,
                })
                .collect(),
            verification: Vec::new(),
            remaining_work: Vec::new(),
            touched_files: SubagentTouchedFiles::default(),
        }
    };

    normalize_outcome(&mut outcome);
    hydrate_artifacts(&mut outcome.artifacts, execution_id, working_dir);
    normalize_outcome(&mut outcome);
    outcome
}

fn extract_fenced_json(section: &str) -> Option<&str> {
    let marker = "```json";
    let start = section.find(marker)?.saturating_add(marker.len());
    let after = section.get(start..)?;
    let end = after.find("```")?;
    after.get(..end).map(str::trim)
}

fn bounded_unique(values: Vec<String>, max_items: usize, max_chars: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    values
        .into_iter()
        .map(|value| bounded_text(value.trim(), max_chars))
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .take(max_items)
        .collect()
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn keep_latest<T>(values: &mut Vec<T>, max_items: usize) {
    let excess = values.len().saturating_sub(max_items);
    if excess > 0 {
        values.drain(..excess);
    }
}

pub(crate) fn normalize_outcome(outcome: &mut SubagentOutcome) {
    outcome.summary = bounded_text(outcome.summary.trim(), DEFAULT_SUMMARY_CHARS);
    keep_latest(&mut outcome.artifacts, MAX_RESULT_ITEMS);
    for artifact in &mut outcome.artifacts {
        artifact.path = bounded_text(artifact.path.trim(), MAX_PATH_CHARS);
        artifact.kind = bounded_text(artifact.kind.trim(), MAX_KIND_CHARS);
    }
    outcome
        .artifacts
        .retain(|artifact| !artifact.path.is_empty());

    keep_latest(&mut outcome.verification, MAX_RESULT_ITEMS);
    for verification in &mut outcome.verification {
        verification.check = bounded_text(verification.check.trim(), MAX_DETAIL_CHARS);
        verification.details = bounded_text(verification.details.trim(), MAX_DETAIL_CHARS);
    }
    outcome
        .verification
        .retain(|verification| !verification.check.is_empty());

    outcome.remaining_work = bounded_unique(
        std::mem::take(&mut outcome.remaining_work),
        MAX_RESULT_ITEMS,
        MAX_DETAIL_CHARS,
    );
    outcome.touched_files.read = bounded_unique(
        std::mem::take(&mut outcome.touched_files.read),
        MAX_RESULT_ITEMS,
        MAX_PATH_CHARS,
    );
    outcome.touched_files.written = bounded_unique(
        std::mem::take(&mut outcome.touched_files.written),
        MAX_RESULT_ITEMS,
        MAX_PATH_CHARS,
    );
}

fn hydrate_artifacts(
    artifacts: &mut [SubagentArtifact],
    execution_id: Option<&str>,
    working_dir: Option<&Path>,
) {
    for artifact in artifacts {
        artifact.producer_execution_id = execution_id.map(str::to_string);
        let raw_path = PathBuf::from(&artifact.path);
        let resolved = if raw_path.is_absolute() {
            raw_path
        } else if let Some(root) = working_dir {
            root.join(raw_path)
        } else {
            raw_path
        };
        let Ok(metadata) = resolved.metadata() else {
            artifact.available = false;
            artifact.bytes = None;
            artifact.sha256 = None;
            continue;
        };
        if !metadata.is_file() {
            artifact.available = false;
            artifact.bytes = None;
            artifact.sha256 = None;
            continue;
        }
        artifact.path = resolved.to_string_lossy().to_string();
        artifact.bytes = Some(metadata.len());
        artifact.sha256 = hash_file(&resolved);
        artifact.available = artifact.sha256.is_some();
    }
}

fn hash_file(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        if let Some(chunk) = buffer.get(..read) {
            hasher.update(chunk);
        } else {
            return None;
        }
    }
    Some(format!("{:x}", hasher.finalize()))
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
        .unwrap_or(body.len());
    body.get(..end)
}

/// Result returned by a subagent execution.
#[derive(Debug, Clone)]
pub struct SubagentResult {
    /// Agent name that produced this result.
    pub agent_name: String,
    /// Final output text (full detail for UI / storage).
    pub output: String,
    /// Runtime-owned structured terminal outcome.
    pub outcome: SubagentOutcome,
    /// Execution duration.
    pub duration: Duration,
    /// Number of iterations used.
    pub iterations: usize,
    /// Token usage (if available).
    pub tokens_used: Option<usize>,
    /// Whether the output was truncated due to token limits.
    pub was_truncated: bool,
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
            outcome: SubagentOutcome {
                status: SubagentStatus::Completed,
                ..SubagentOutcome::default()
            },
            duration,
            iterations: 1,
            tokens_used: None,
            was_truncated: false,
            mode: ExecutionMode::Sync,
            isolation_observed: ObservedIsolation::Unknown,
            usage: None,
        }
        .with_structured(None, None)
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
            outcome: SubagentOutcome {
                status: SubagentStatus::Completed,
                ..SubagentOutcome::default()
            },
            duration,
            iterations,
            tokens_used: None,
            was_truncated: false,
            mode: ExecutionMode::Fork,
            isolation_observed: ObservedIsolation::Unknown,
            usage: None,
        }
        .with_structured(None, None)
    }

    /// Fill the structured outcome from the explicit result contract or fallback text.
    pub fn with_structured(
        mut self,
        execution_id: Option<&str>,
        working_dir: Option<&Path>,
    ) -> Self {
        let status = self.outcome.status;
        self.outcome = parse_subagent_outcome(&self.output, status, execution_id, working_dir);
        self
    }

    pub fn cancelled(
        agent_name: impl Into<String>,
        output: impl Into<String>,
        mode: ExecutionMode,
    ) -> Self {
        let output = output.into();
        Self {
            agent_name: agent_name.into(),
            outcome: SubagentOutcome::terminal(
                SubagentStatus::Cancelled,
                output.clone(),
                vec![output.clone()],
            ),
            output,
            duration: Duration::ZERO,
            iterations: 0,
            tokens_used: None,
            was_truncated: false,
            mode,
            isolation_observed: ObservedIsolation::Unknown,
            usage: None,
        }
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
            .chain(std::iter::repeat_n('x', 2000))
            .collect();
        let (summary, _) = split_subagent_output(&raw);
        assert_eq!(summary.chars().count(), DEFAULT_SUMMARY_CHARS);
        // Must not panic and must start with the multi-byte prefix.
        assert!(summary.starts_with("中文"));
    }

    #[test]
    fn test_team_spec_construction() {
        // Sprint 11: a TeamSpec with ManagerSubagent strategy can be constructed
        // and attached to a SubagentDefinition. Subagents are name-references.
        use super::super::team::TeamConfig;
        use super::super::team::strategy::TeamStrategy;
        let spec = TeamSpec {
            strategy: TeamStrategy::ManagerSubagent,
            manager: "planner".to_string(),
            subagents: vec!["explorer".to_string(), "summarizer".to_string()],
            config: TeamConfig::default(),
        };
        let mut def = SubagentDefinition::new("team-research", "team research subagent");
        assert!(def.team.is_none());
        def.team = Some(spec.clone());
        assert_eq!(def.team.as_ref().unwrap().manager, "planner");
        assert_eq!(def.team.as_ref().unwrap().subagents.len(), 2);
        assert_eq!(
            def.team.as_ref().unwrap().strategy,
            TeamStrategy::ManagerSubagent
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
        let def = SubagentDefinition::simple_sync("subagent");
        assert_eq!(def.name, "subagent");
        assert!(def.description.contains("subagent"));
    }

    #[test]
    fn test_subagent_result_sync() {
        let result = SubagentResult::sync_result("a", "ok".into(), Duration::from_millis(100));
        assert_eq!(result.mode, ExecutionMode::Sync);
        assert_eq!(result.output, "ok");
        assert_eq!(result.outcome.status, SubagentStatus::Completed);
    }

    #[test]
    fn structured_result_ignores_model_status_and_hydrates_artifact() -> Result<(), String> {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let artifact_path = dir.path().join("report.txt");
        std::fs::write(&artifact_path, "result").map_err(|error| error.to_string())?;
        let raw = "## Result\n```json\n{\"contract_version\":1,\"status\":\"completed\",\"summary\":\"done\",\"artifacts\":[{\"path\":\"report.txt\",\"kind\":\"report\"}],\"verification\":[{\"check\":\"cargo test\",\"status\":\"passed\",\"source\":\"observed\"}],\"remaining_work\":[],\"touched_files\":{\"read\":[],\"written\":[\"report.txt\"]}}\n```";
        let outcome = parse_subagent_outcome(
            raw,
            SubagentStatus::TimedOut,
            Some("task-1:1"),
            Some(dir.path()),
        );
        assert_eq!(outcome.status, SubagentStatus::TimedOut);
        assert_eq!(outcome.contract_version, 1);
        let artifact = outcome
            .artifacts
            .first()
            .ok_or_else(|| "artifact missing".to_string())?;
        assert!(artifact.available);
        assert_eq!(artifact.sha256.as_deref().map(str::len), Some(64));
        assert_eq!(artifact.producer_execution_id.as_deref(), Some("task-1:1"));
        assert!(matches!(
            outcome.verification.first(),
            Some(SubagentVerification {
                source: SubagentVerificationSource::Reported,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn structured_result_bounds_utf8_evidence() -> Result<(), String> {
        let long = "中".repeat(600);
        let remaining_work: Vec<String> = (0..70).map(|index| format!("{index}-{long}")).collect();
        let payload = serde_json::json!({
            "contract_version": 1,
            "status": "completed",
            "summary": long.repeat(3),
            "artifacts": [],
            "verification": [],
            "remaining_work": remaining_work,
            "touched_files": { "read": [], "written": [] }
        });
        let raw = format!("## Result\n```json\n{payload}\n```");
        let outcome = parse_subagent_outcome(&raw, SubagentStatus::Completed, None, None);

        assert_eq!(outcome.summary.chars().count(), DEFAULT_SUMMARY_CHARS);
        assert_eq!(outcome.remaining_work.len(), MAX_RESULT_ITEMS);
        assert!(
            outcome
                .remaining_work
                .iter()
                .all(|item| item.chars().count() <= MAX_DETAIL_CHARS)
        );
        Ok(())
    }

    #[test]
    fn rendered_result_contract_round_trips_through_parser() {
        let rendered = render_result_contract();
        let outcome = parse_subagent_outcome(
            &rendered,
            SubagentStatus::Completed,
            Some("round-trip:1"),
            None,
        );

        assert_eq!(outcome.contract_version, SUBAGENT_RESULT_CONTRACT_VERSION);
        assert_eq!(outcome.status, SubagentStatus::Completed);
        assert_eq!(outcome.summary, "at most 1200 characters");
        assert!(matches!(
            outcome.verification.first(),
            Some(SubagentVerification {
                source: SubagentVerificationSource::Reported,
                ..
            })
        ));
    }
}
