//! Subagent core types — definitions, execution modes, and results

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::usage::LlmUsageStats;
use echo_core::agent::ExecutionUsage;

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
    /// Team intent compiled into the canonical revisioned task DAG runtime.
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

/// Product-neutral name of the isolation boundary established for a dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObservedIsolation(String);

impl ObservedIsolation {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let value = value.trim();
        if value.is_empty() {
            Self::default()
        } else {
            Self(value.chars().take(MAX_KIND_CHARS).collect())
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ObservedIsolation {
    fn default() -> Self {
        Self("unknown".to_string())
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

/// Mutation authority declared by a Subagent definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentAccessMode {
    ReadOnly,
    Write,
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
    /// Typed mutation boundary. Tags are discovery metadata and must never be
    /// decoded back into this authority.
    pub access_mode: SubagentAccessMode,
    /// Default execution mode when dispatched.
    pub execution_mode: ExecutionMode,
    /// Model override (None = inherit from parent).
    pub model: Option<String>,
    /// Reasoning-depth override for this subagent, in
    /// `echo_core::llm::ThinkingConfig::parse_spec` syntax (`low`/`medium`/
    /// `high`/`disabled`/budget number; `auto`/empty = model default).
    /// `None` = inherit the parent generation's thinking. Used by cheap
    /// long-running roles that intentionally use reduced reasoning depth.
    pub thinking: Option<String>,
    /// System prompt override (None = inherit or auto-generate).
    pub system_prompt: Option<String>,
    /// Restrict available tools by name (None = inherit all from parent).
    pub tool_filter: Option<Vec<String>>,
    /// Maximum agent iterations. `None` leaves the concrete factory's bounded
    /// default in effect; a `ReactAgentBuilder` rejects an explicit zero.
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
    /// Whether this subagent uses the lightweight (infrastructure-sharing) mode.
    ///
    /// When `true`, the subagent shares the parent's LLM client, ToolManager,
    /// and GuardManager instead of creating new instances.
    pub lightweight: bool,
    /// Optional product-owned isolation kind resolved by an injected provider.
    pub isolation: Option<String>,
    /// Declarative Team intent compiled by the canonical task runtime.
    pub team: Option<super::team::TeamSpec>,
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
            access_mode: SubagentAccessMode::Write,
            execution_mode: ExecutionMode::Sync,
            model: None,
            thinking: None,
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
            isolation: None,
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
pub const SUBAGENT_RESULT_CONTRACT_VERSION: u32 = 2;
const MAX_RESULT_ITEMS: usize = 64;
const MAX_DETAIL_CHARS: usize = 500;
const MAX_PATH_CHARS: usize = 2048;
const MAX_KIND_CHARS: usize = 80;
const MAX_ATTRIBUTES_CHARS: usize = 4_000;

/// Runtime-owned terminal status for one subagent dispatch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Running,
    Completed,
    #[default]
    Failed,
    Cancelled,
    TimedOut,
}

impl SubagentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}

impl std::str::FromStr for SubagentStatus {
    type Err = String;

    /// Parse the stable wire spelling used by runtime projections.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "timed_out" => Ok(Self::TimedOut),
            _ => Err(format!("unknown Subagent status: {value}")),
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

/// Whether evidence came from observed runtime behavior or subagent output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentEvidenceSource {
    Observed,
    #[default]
    Reported,
}

/// Product-neutral evidence emitted or observed during a subagent dispatch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentEvidence {
    pub kind: String,
    pub subject: String,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub details: String,
    #[serde(default)]
    pub source: SubagentEvidenceSource,
    #[serde(default)]
    pub attributes: serde_json::Value,
}

/// Verification check derived from structured subagent evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentVerification {
    pub check: String,
    pub status: SubagentVerificationStatus,
    pub details: String,
    pub source: SubagentEvidenceSource,
}

/// Outcome of a verification check observed or reported by a subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentVerificationStatus {
    Passed,
    Failed,
    NotRun,
}

/// Files referenced by observed subagent tool evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentTouchedFiles {
    pub read: Vec<String>,
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
    pub evidence: Vec<SubagentEvidence>,
    /// Verification views derived from the generic evidence stream.
    #[serde(default)]
    pub verification: Vec<SubagentVerification>,
    #[serde(default)]
    pub remaining_work: Vec<String>,
    /// Read/write paths derived from observed tool evidence.
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
            evidence: Vec::new(),
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
        let summary = bounded_text(summary.into().trim(), DEFAULT_SUMMARY_CHARS);
        let remaining_work = bounded_unique(remaining_work, MAX_RESULT_ITEMS, MAX_DETAIL_CHARS);
        Self {
            contract_version: SUBAGENT_RESULT_CONTRACT_VERSION,
            status,
            summary,
            artifacts: Vec::new(),
            evidence: Vec::new(),
            verification: Vec::new(),
            remaining_work,
            touched_files: SubagentTouchedFiles::default(),
        }
    }

    pub fn is_completed(&self) -> bool {
        self.status == SubagentStatus::Completed
    }

    /// Refresh generic verification and file-access views from evidence.
    pub fn refresh_derived_views(&mut self) {
        self.verification = self
            .evidence
            .iter()
            .filter_map(project_verification_evidence)
            .collect();
        let mut touched = SubagentTouchedFiles::default();
        for evidence in &self.evidence {
            let Some((write, path)) = project_tool_file_access(evidence) else {
                continue;
            };
            let target = if write {
                &mut touched.written
            } else {
                &mut touched.read
            };
            if !target.iter().any(|existing| existing == &path) {
                target.push(path);
            }
        }
        self.touched_files = touched;
    }

    /// Borrow the derived verification view without rebuilding it.
    pub fn verification(&self) -> &[SubagentVerification] {
        &self.verification
    }

    /// Borrow the derived read/write file view without rebuilding it.
    pub fn touched_files(&self) -> &SubagentTouchedFiles {
        &self.touched_files
    }
}

fn project_verification_evidence(evidence: &SubagentEvidence) -> Option<SubagentVerification> {
    let check = if evidence.kind == "verification" {
        Some(evidence.subject.clone())
    } else if evidence.kind == "tool_result" {
        evidence
            .attributes
            .get("args")
            .and_then(|args| verification_check_from_tool(&evidence.subject, args))
    } else {
        None
    }?;
    Some(SubagentVerification {
        check,
        status: match evidence.outcome.as_deref() {
            Some("passed" | "succeeded") => SubagentVerificationStatus::Passed,
            Some("failed") => SubagentVerificationStatus::Failed,
            _ => SubagentVerificationStatus::NotRun,
        },
        details: evidence.details.clone(),
        source: evidence.source,
    })
}

fn verification_check_from_tool(name: &str, args: &serde_json::Value) -> Option<String> {
    let normalized = name.to_ascii_lowercase().replace('-', "_");
    if !matches!(
        normalized.as_str(),
        "shell" | "bash" | "terminal" | "run_code" | "execute_command"
    ) {
        return None;
    }
    ["command", "cmd", "code", "script"]
        .iter()
        .find_map(|key| args.get(*key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn project_tool_file_access(evidence: &SubagentEvidence) -> Option<(bool, String)> {
    let normalized = evidence.subject.to_ascii_lowercase().replace('-', "_");
    let write = normalized.contains("write")
        || normalized.contains("edit")
        || normalized.contains("delete")
        || normalized.contains("patch");
    let read = normalized.contains("read")
        || normalized.contains("search")
        || normalized.contains("glob")
        || normalized.contains("grep");
    if !write && !read {
        return None;
    }
    let args = evidence.attributes.get("args")?;
    ["path", "file_path", "target", "directory"]
        .iter()
        .find_map(|key| args.get(*key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|path| (write, path.to_string()))
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
\"kind\":\"product-defined kind\"}}],\"evidence\":[{{\"kind\":\"product-defined kind\",\
\"subject\":\"what was observed\",\"outcome\":\"optional bounded outcome\",\
\"details\":\"bounded evidence\",\"source\":\"reported\",\"attributes\":{{}}}}],\"remaining_work\":[]}}\n\
```\n\
Runtime owns terminal status and observed evidence. Report only real artifacts and evidence; put incomplete or blocked work in remaining_work."
    )
}

/// Parse fenced or standalone JSON objects embedded in a subagent response.
/// Product layers may inspect named optional fields while the terminal
/// `## Result` object remains parsed by [`parse_subagent_outcome`].
pub fn parse_json_objects(raw: &str) -> Vec<serde_json::Value> {
    let mut objects = Vec::new();
    for block in raw.split("```json").skip(1) {
        if let Some(json) = block.split("```").next()
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(json.trim())
            && value.is_object()
        {
            objects.push(value);
        }
    }
    let trimmed = raw.trim();
    if trimmed.starts_with('{')
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
        && value.is_object()
    {
        objects.push(value);
    }
    objects
}

#[derive(Debug, Deserialize)]
struct ReportedSubagentOutcome {
    #[serde(default)]
    contract_version: u32,
    summary: String,
    #[serde(default)]
    artifacts: Vec<SubagentArtifact>,
    #[serde(default)]
    evidence: Vec<SubagentEvidence>,
    #[serde(default)]
    remaining_work: Vec<String>,
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
    _execution_id: Option<&str>,
    _working_dir: Option<&Path>,
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
            evidence: reported
                .evidence
                .into_iter()
                .map(|mut evidence| {
                    // Only runtime-observed tool events may create observed evidence.
                    evidence.source = SubagentEvidenceSource::Reported;
                    evidence
                })
                .collect(),
            verification: Vec::new(),
            remaining_work: bounded_unique(
                reported.remaining_work,
                MAX_RESULT_ITEMS,
                MAX_DETAIL_CHARS,
            ),
            touched_files: SubagentTouchedFiles::default(),
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
                    producer_execution_id: None,
                    available: false,
                })
                .collect(),
            evidence: Vec::new(),
            verification: Vec::new(),
            remaining_work: Vec::new(),
            touched_files: SubagentTouchedFiles::default(),
        }
    };

    normalize_outcome(&mut outcome);
    outcome.refresh_derived_views();
    for artifact in &mut outcome.artifacts {
        artifact.bytes = None;
        artifact.sha256 = None;
        artifact.producer_execution_id = None;
        artifact.available = false;
    }
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

    keep_latest(&mut outcome.evidence, MAX_RESULT_ITEMS);
    for evidence in &mut outcome.evidence {
        evidence.kind = bounded_text(evidence.kind.trim(), MAX_KIND_CHARS);
        evidence.subject = bounded_text(evidence.subject.trim(), MAX_PATH_CHARS);
        evidence.outcome = evidence
            .outcome
            .take()
            .map(|outcome| bounded_text(outcome.trim(), MAX_DETAIL_CHARS))
            .filter(|outcome| !outcome.is_empty());
        evidence.details = bounded_text(evidence.details.trim(), MAX_DETAIL_CHARS);
        if evidence.attributes.to_string().chars().count() > MAX_ATTRIBUTES_CHARS {
            evidence.attributes = serde_json::json!({ "truncated": true });
        }
    }
    outcome
        .evidence
        .retain(|evidence| !evidence.kind.is_empty() && !evidence.subject.is_empty());

    outcome.remaining_work = bounded_unique(
        std::mem::take(&mut outcome.remaining_work),
        MAX_RESULT_ITEMS,
        MAX_DETAIL_CHARS,
    );
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
    pub llm_usage: Option<LlmUsageStats>,
}

impl SubagentResult {
    /// Return the stable usage facts for persistence or reporting.
    pub fn usage(&self) -> ExecutionUsage {
        let duration_ms = u64::try_from(self.duration.as_millis()).unwrap_or(u64::MAX);
        let iterations = u64::try_from(self.iterations).unwrap_or(u64::MAX);
        ExecutionUsage {
            duration_ms: Some(duration_ms),
            tokens_used: self
                .llm_usage
                .as_ref()
                .map(|usage| usage.prompt_tokens.saturating_add(usage.completion_tokens)),
            iterations: Some(iterations),
        }
    }

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
            isolation_observed: ObservedIsolation::default(),
            llm_usage: None,
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
            isolation_observed: ObservedIsolation::default(),
            llm_usage: None,
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
            isolation_observed: ObservedIsolation::default(),
            llm_usage: None,
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
    use crate::agent::subagent::{TeamConfig, TeamSpec, TeamStrategy};

    #[test]
    fn test_execution_mode_default() {
        assert_eq!(ExecutionMode::default(), ExecutionMode::Sync);
    }

    #[test]
    fn test_execution_mode_display() {
        assert_eq!(ExecutionMode::Sync.to_string(), "sync");
        assert_eq!(ExecutionMode::Fork.to_string(), "fork");
        assert_eq!(ExecutionMode::Teammate.to_string(), "teammate");
        assert_eq!(ExecutionMode::Team.to_string(), "team");

        let spec = TeamSpec {
            strategy: TeamStrategy::ManagerSubagent,
            manager: "planner".to_string(),
            subagents: vec!["researcher".to_string()],
            config: TeamConfig::default(),
        };
        let definition = SubagentDefinition {
            team: Some(spec),
            execution_mode: ExecutionMode::Team,
            ..SubagentDefinition::new("review-team", "Review Team")
        };
        assert!(definition.team.is_some());
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
        let mut result = SubagentResult::sync_result("a", "ok".into(), Duration::from_millis(100));
        result.iterations = 3;
        result.llm_usage = Some(LlmUsageStats {
            prompt_tokens: 7,
            completion_tokens: 5,
            ..LlmUsageStats::default()
        });
        assert_eq!(result.mode, ExecutionMode::Sync);
        assert_eq!(result.output, "ok");
        assert_eq!(result.outcome.status, SubagentStatus::Completed);
        assert_eq!(
            result.usage(),
            ExecutionUsage {
                duration_ms: Some(100),
                tokens_used: Some(12),
                iterations: Some(3),
            }
        );
    }

    #[test]
    fn structured_result_does_not_attest_model_reported_artifact() -> Result<(), String> {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let artifact_path = dir.path().join("report.txt");
        std::fs::write(&artifact_path, "result").map_err(|error| error.to_string())?;
        let raw = "## Result\n```json\n{\"contract_version\":2,\"status\":\"completed\",\"summary\":\"done\",\"artifacts\":[{\"path\":\"report.txt\",\"kind\":\"report\"}],\"evidence\":[{\"kind\":\"verification\",\"subject\":\"cargo test\",\"outcome\":\"passed\",\"source\":\"observed\"}],\"remaining_work\":[]}\n```";
        let outcome = parse_subagent_outcome(
            raw,
            SubagentStatus::TimedOut,
            Some("task-1:1"),
            Some(dir.path()),
        );
        assert_eq!(outcome.status, SubagentStatus::TimedOut);
        assert_eq!(outcome.contract_version, 2);
        let artifact = outcome
            .artifacts
            .first()
            .ok_or_else(|| "artifact missing".to_string())?;
        assert!(!artifact.available);
        assert!(artifact.sha256.is_none());
        assert!(artifact.producer_execution_id.is_none());
        assert!(matches!(
            outcome.evidence.first(),
            Some(SubagentEvidence {
                source: SubagentEvidenceSource::Reported,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn structured_outcome_exposes_generic_evidence_views() -> Result<(), String> {
        let raw = "## Result\n```json\n{\"contract_version\":2,\"status\":\"completed\",\"summary\":\"done\",\"artifacts\":[],\"evidence\":[{\"kind\":\"verification\",\"subject\":\"cargo check\",\"outcome\":\"passed\",\"details\":\"ok\"},{\"kind\":\"tool_result\",\"subject\":\"write_file\",\"outcome\":\"succeeded\",\"attributes\":{\"args\":{\"path\":\"src/lib.rs\"}}}],\"remaining_work\":[]}\n```";
        let outcome = parse_subagent_outcome(raw, SubagentStatus::Completed, None, None);
        assert_eq!(outcome.verification().len(), 1);
        assert_eq!(outcome.touched_files().written, vec!["src/lib.rs"]);
        Ok(())
    }

    #[test]
    fn structured_result_bounds_utf8_evidence() -> Result<(), String> {
        let long = "中".repeat(600);
        let remaining_work: Vec<String> = (0..70).map(|index| format!("{index}-{long}")).collect();
        let payload = serde_json::json!({
            "contract_version": 2,
            "status": "completed",
            "summary": long.repeat(3),
            "artifacts": [],
            "evidence": [],
            "remaining_work": remaining_work,
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
            outcome.evidence.first(),
            Some(SubagentEvidence {
                source: SubagentEvidenceSource::Reported,
                ..
            })
        ));
    }
}
