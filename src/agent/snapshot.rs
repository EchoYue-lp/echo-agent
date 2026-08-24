//! Agent execution snapshot — captures agent state for `'static` streaming.
//!
//! [`AgentRunSnapshot`] replaces the old 33-field manual `AgentSnapshot` clone
//! in `stream_channel.rs` with a composition-based approach. Configuration,
//! tool runtime, and guard runtime are each wrapped in `Arc` so the snapshot
//! is cheap to clone and safe to move into a `tokio::spawn` future.

use crate::agent::AgentCallback;
use crate::agent::InterventionCallback;
use crate::audit::AuditLogger;
use crate::memory::snapshot::SnapshotManager;
use crate::skills::hooks::HookRegistry;
use crate::tools::{ToolExecutionConfig, ToolFailure, ToolManager, ToolResult};
use crate::trace::{RunEvent, RunStatus, RunStore};
use echo_core::circuit_breaker::CircuitBreaker;
use echo_core::llm::types::{Message, Role};
use echo_core::tokenizer::Tokenizer;
use std::sync::Arc;

const TOOL_OUTPUT_PREVIEW_CHARS: usize = 500;
const TOOL_OUTPUT_SPILL_FAILURE_FALLBACK_TOKENS: usize = 8_000;

/// Result of applying the run-scoped output budget to one tool result.
pub(crate) struct ProcessedToolOutput {
    pub output: String,
    pub truncated: bool,
    pub metadata: std::collections::HashMap<String, String>,
}

pub(crate) struct ToolCallFailure {
    pub name: String,
    pub error: crate::error::ReactError,
    pub result: ToolResult,
}

pub(crate) struct ToolCallSuccess {
    pub name: String,
    pub result: ToolResult,
}

fn is_internal_transcript_message(message: &Message) -> bool {
    if crate::compression::is_context_projection_message(message) {
        return true;
    }
    let Some(text) = message.content.as_text() else {
        return false;
    };
    let trimmed = text.trim_start();

    match message.role {
        Role::System => true,
        Role::User => {
            trimmed.starts_with("[Relevant historical memories]")
                || trimmed.starts_with("[The above memories")
                || trimmed.starts_with("[Verifier feedback]")
                || trimmed.starts_with("[Hook:")
                || trimmed.starts_with("[Memory")
                || trimmed.starts_with("[Context")
                || trimmed.starts_with("[Compact")
                || trimmed.starts_with("[Compression")
        }
        Role::Tool => {
            trimmed.starts_with("[placeholder]")
                || trimmed.starts_with("[synthetic]")
                || trimmed.contains("placeholder result")
        }
        Role::Assistant | Role::Custom(_) => false,
    }
}

fn filter_user_visible_transcript(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter(|message| !is_internal_transcript_message(message))
        .cloned()
        .collect()
}

fn same_transcript_message(
    left: &crate::memory::StoredMessage,
    right: &crate::memory::StoredMessage,
) -> bool {
    left.role == right.role
        && left.content == right.content
        && left.attachments_json == right.attachments_json
        && left.tool_calls_json == right.tool_calls_json
        && left.tool_result_json == right.tool_result_json
}

fn merge_transcript_projection(
    mut persisted: Vec<crate::memory::StoredMessage>,
    projected: Vec<crate::memory::StoredMessage>,
) -> Vec<crate::memory::StoredMessage> {
    if persisted.is_empty() {
        return projected;
    }
    if projected.is_empty() {
        return persisted;
    }

    // The active view may be a compacted suffix, or it may retain the full
    // conversation while replacing old tool traces in the middle. Anchor on
    // the last durable message, then choose the occurrence with the longest
    // backward match. Only the active suffix after that boundary is new.
    let Some(last_persisted) = persisted.last() else {
        return projected;
    };
    let mut best_boundary: Option<(usize, usize)> = None;
    for (end_index, candidate) in projected.iter().enumerate() {
        if !same_transcript_message(last_persisted, candidate) {
            continue;
        }
        let matched = persisted
            .iter()
            .rev()
            .zip(projected.iter().take(end_index.saturating_add(1)).rev())
            .take_while(|(left, right)| same_transcript_message(left, right))
            .count();
        let replace = best_boundary.is_none_or(|(best_matched, best_end)| {
            matched > best_matched || (matched == best_matched && end_index > best_end)
        });
        if replace {
            best_boundary = Some((matched, end_index));
        }
    }

    let append_from = best_boundary
        .map(|(_, end_index)| end_index.saturating_add(1))
        .unwrap_or(0);
    persisted.extend(projected.into_iter().skip(append_from));
    persisted
}

fn same_projection_content(
    left: &crate::state::TranscriptProjectionMessage,
    right: &crate::state::TranscriptProjectionMessage,
) -> bool {
    left.digest == right.digest
}

fn generation_overlap(
    previous: &[crate::state::TranscriptProjectionMessage],
    projected: &[crate::state::TranscriptProjectionMessage],
) -> usize {
    let max_overlap = previous.len().min(projected.len());
    (0..=max_overlap)
        .rev()
        .find(|overlap| {
            previous
                .iter()
                .skip(previous.len().saturating_sub(*overlap))
                .zip(projected.iter().take(*overlap))
                .all(|(left, right)| same_projection_content(left, right))
        })
        .unwrap_or_default()
}

fn transcript_projection_messages(
    projected: &[crate::memory::StoredMessage],
) -> crate::error::Result<Vec<crate::state::TranscriptProjectionMessage>> {
    projected
        .iter()
        .map(|message| {
            use sha2::{Digest, Sha256};
            let normalized = serde_json::to_vec(&(
                &message.role,
                &message.content,
                crate::memory::normalized_transcript_attachments(message)?,
                &message.tool_calls_json,
                &message.tool_result_json,
            ))?;
            Ok(crate::state::TranscriptProjectionMessage {
                ordinal: 0,
                digest: format!("{:x}", Sha256::digest(normalized)),
            })
        })
        .collect()
}

#[derive(Clone, Default)]
pub(crate) struct TranscriptProjectionCursor {
    generation_id: Option<String>,
    next_ordinal: u64,
    projected: Vec<crate::state::TranscriptProjectionMessage>,
}

impl TranscriptProjectionCursor {
    pub(crate) fn align_restored(
        &mut self,
        generation_id: &str,
        messages: &[Message],
    ) -> crate::error::Result<()> {
        let visible = filter_user_visible_transcript(messages);
        let projected = crate::memory::project_messages(generation_id, &visible)?;
        let mut assigned = transcript_projection_messages(&projected)?;
        self.next_ordinal = 0;
        for message in &mut assigned {
            message.ordinal = self.next_ordinal;
            self.next_ordinal = self.next_ordinal.checked_add(1).ok_or_else(|| {
                crate::error::ReactError::Other(
                    "transcript projection ordinal capacity exhausted".to_string(),
                )
            })?;
        }
        self.projected = assigned;
        self.generation_id = Some(generation_id.to_string());
        Ok(())
    }

    fn assign(
        &mut self,
        generation_id: &str,
        projected: &[crate::memory::StoredMessage],
    ) -> crate::error::Result<Vec<crate::state::TranscriptProjectionMessage>> {
        if self.generation_id.as_deref() != Some(generation_id) {
            self.generation_id = Some(generation_id.to_string());
            self.next_ordinal = 0;
            self.projected.clear();
        }
        let identities = transcript_projection_messages(projected)?;
        let overlap = generation_overlap(&self.projected, &identities);
        let mut assigned = self
            .projected
            .iter()
            .skip(self.projected.len().saturating_sub(overlap))
            .cloned()
            .collect::<Vec<_>>();
        for mut message in identities.into_iter().skip(overlap) {
            message.ordinal = self.next_ordinal;
            self.next_ordinal = self.next_ordinal.checked_add(1).ok_or_else(|| {
                crate::error::ReactError::Other(
                    "transcript projection ordinal capacity exhausted".to_string(),
                )
            })?;
            assigned.push(message);
        }
        Ok(assigned)
    }

    fn checkpoint_for(&self, generation_id: &str) -> crate::state::TranscriptProjectionCheckpoint {
        crate::state::TranscriptProjectionCheckpoint {
            generation_id: generation_id.to_string(),
            next_ordinal: if self.generation_id.as_deref() == Some(generation_id) {
                self.next_ordinal
            } else {
                0
            },
            projected: if self.generation_id.as_deref() == Some(generation_id) {
                self.projected.clone()
            } else {
                Vec::new()
            },
        }
    }

    pub(crate) fn restore(&mut self, checkpoint: crate::state::TranscriptProjectionCheckpoint) {
        self.generation_id = Some(checkpoint.generation_id);
        self.next_ordinal = checkpoint.next_ordinal;
        self.projected = checkpoint.projected;
    }
}

fn merge_generation_projection(
    mut persisted: Vec<crate::memory::StoredMessage>,
    projected: &[crate::memory::StoredMessage],
    assigned: &[crate::state::TranscriptProjectionMessage],
    generation_id: &str,
) -> crate::error::Result<Vec<crate::memory::StoredMessage>> {
    if projected.len() != assigned.len() {
        return Err(crate::error::ReactError::Other(
            "transcript projection assignment length mismatch".to_string(),
        ));
    }
    let mut persisted_ordinals =
        std::collections::HashMap::<u64, crate::state::TranscriptProjectionMessage>::new();
    for message in &persisted {
        let Some(meta) = crate::memory::transcript_projection_meta(message)? else {
            continue;
        };
        if meta.generation_id != generation_id {
            continue;
        }
        let mut identity = transcript_projection_messages(std::slice::from_ref(message))?
            .pop()
            .ok_or_else(|| {
                crate::error::ReactError::Other(
                    "transcript projection identity was unexpectedly empty".to_string(),
                )
            })?;
        identity.ordinal = meta.ordinal;
        if let Some(existing) = persisted_ordinals.get(&meta.ordinal) {
            if existing != &identity {
                return Err(crate::error::ReactError::Other(format!(
                    "transcript generation ordinal {} has conflicting content",
                    meta.ordinal
                )));
            }
        } else {
            persisted_ordinals.insert(meta.ordinal, identity);
        }
    }
    for (mut message, identity) in projected.iter().cloned().zip(assigned.iter()) {
        match persisted_ordinals.get(&identity.ordinal) {
            Some(existing) if existing == identity => continue,
            Some(_) => {
                return Err(crate::error::ReactError::Other(format!(
                    "transcript generation ordinal {} collided with different content",
                    identity.ordinal
                )));
            }
            None => {}
        }
        crate::memory::set_transcript_projection_meta(
            &mut message,
            generation_id,
            identity.ordinal,
        )?;
        persisted.push(message);
    }
    Ok(persisted)
}

// ── RuntimeConfig ────────────────────────────────────────────────────

/// Immutable subset of [`AgentConfig`](crate::agent::AgentConfig) that
/// does not change during a streaming run.
#[derive(Clone)]
pub struct RuntimeConfig {
    pub agent_name: String,
    pub model_name: String,
    pub provider: Option<String>,
    pub max_iterations: usize,
    pub token_limit: usize,
    /// Construction-time validation failure for the configured token budget.
    pub token_budget_error: Option<String>,
    pub run_budget: echo_core::agent::RunBudgetPolicy,
    pub supports_tool_choice_none: bool,
    /// Input modalities accepted by the configured model. `None` preserves
    /// compatibility for custom agents that do not provide an
    /// [`crate::config::LlmConfig`].
    pub input_modalities: Option<Vec<echo_core::llm::ModelInputModality>>,
    pub session_id: Option<String>,
    /// Identity used exclusively for `RuntimeStateStore` checkpoints.
    pub runtime_state_id: Option<String>,
    pub conversation_id: Option<String>,
    /// Session-bound working directory (worktree path). Injected into each
    /// tool call's ToolContext so file/shell/git tools run inside the
    /// isolated checkout. None = use process cwd (backward compatible).
    pub working_dir: Option<std::path::PathBuf>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub tool_error_feedback: bool,
    pub force_read_before_edit: bool,
    pub enable_tool: bool,
    pub llm_max_retries: usize,
    pub llm_retry_delay_ms: u64,
    pub max_tool_output_tokens: Option<usize>,
    pub tool_output_artifacts: Option<echo_core::tools::artifact::ToolOutputArtifactConfig>,
    pub tool_execution: ToolExecutionConfig,
    pub callbacks: Vec<Arc<dyn AgentCallback>>,
    /// How often to save runtime checkpoints (0 = only at end, N = every N iterations).
    pub react_checkpoint_interval: usize,
    /// Whether the verifier is enabled.
    pub verifier_enabled: bool,
    /// Minimum score for verifier to pass.
    pub verifier_min_score: f64,
    /// Maximum verifier retry attempts.
    pub verifier_max_retries: usize,
    /// Whether plan mode is enabled (read-only tools only).
    pub plan_mode: bool,
    /// Stable user identifier for KVCache isolation (DeepSeek, etc.).
    pub cache_user_id: Option<String>,
}

impl RuntimeConfig {
    /// Create a snapshot from the agent's config.
    pub fn from_agent_config(config: &crate::agent::AgentConfig) -> Self {
        Self {
            agent_name: config.agent_name.clone(),
            model_name: config.model_name.clone(),
            provider: config
                .model_profile
                .as_ref()
                .map(|profile| profile.provider.clone()),
            max_iterations: config.max_iterations,
            token_limit: config.token_limit,
            token_budget_error: config
                .token_budget_config
                .enabled
                .then(|| config.token_budget_config.build(config.token_limit).err())
                .flatten()
                .map(|error| error.to_string()),
            run_budget: config.run_budget.clone(),
            supports_tool_choice_none: config
                .model_profile
                .as_ref()
                .is_none_or(|profile| profile.supports_tool_choice_none),
            input_modalities: None,
            session_id: config.session_id.clone(),
            runtime_state_id: config.conversation_id.clone(),
            conversation_id: config.conversation_id.clone(),
            working_dir: config.working_dir.lock().ok().and_then(|g| g.clone()),
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            tool_error_feedback: config.tool_error_feedback,
            force_read_before_edit: config.force_read_before_edit,
            enable_tool: config.enable_tool,
            llm_max_retries: config.llm_max_retries,
            llm_retry_delay_ms: config.llm_retry_delay_ms,
            max_tool_output_tokens: config.max_tool_output_tokens,
            tool_output_artifacts: config.get_tool_output_artifacts(),
            tool_execution: config.tool_execution.clone(),
            callbacks: config.callbacks.to_vec(),
            react_checkpoint_interval: config.react_checkpoint_interval,
            verifier_enabled: config.verifier_enabled,
            verifier_min_score: config.verifier_min_score,
            verifier_max_retries: config.verifier_max_retries,
            plan_mode: config.plan_mode,
            cache_user_id: config.cache_user_id.clone(),
        }
    }

    /// Return the session ID as a &str, defaulting to empty.
    pub fn session_id_str(&self) -> &str {
        self.session_id.as_deref().unwrap_or("")
    }
}

// ── ToolRuntime ──────────────────────────────────────────────────────

/// Tool execution state (tools, hooks, interventions). Shared via `Arc`.
#[derive(Clone)]
pub struct ToolRuntime {
    pub tool_manager: Arc<ToolManager>,
    pub hook_registry: Arc<tokio::sync::RwLock<HookRegistry>>,
    pub intervention_callbacks: Vec<Arc<dyn InterventionCallback>>,
    /// Allowed tool patterns from activated skills (captured at snapshot time).
    /// `None` = unrestricted (no skill restricts tools).
    pub skill_allowed_tools: Option<std::collections::HashSet<String>>,
    /// Names of all activated skills (captured at snapshot time).
    pub active_skill_names: Vec<String>,
    /// Current plan state (shared with ReactAgent).
    pub plan_state: Arc<tokio::sync::RwLock<Option<String>>>,
    /// Effective disabled tools captured for this invocation.
    pub disabled_tools: std::collections::HashSet<String>,
    /// Mutable schema visibility for deferred tools in this invocation.
    pub visibility: Option<std::sync::Arc<echo_core::tools::ToolVisibilityState>>,
    /// Whether the invocation uses plan mode's read-only tool surface.
    pub plan_mode: bool,
}

impl ToolRuntime {
    pub fn from_agent(
        agent: &super::ReactAgent,
        invocation_disabled_tools: Option<&std::collections::HashSet<String>>,
        invocation_visible_tools: Option<&std::collections::HashSet<String>>,
    ) -> Self {
        let mut disabled_tools = agent
            .tools
            .disabled_tools
            .read()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_default();
        if let Some(profile) = agent.config.model_profile.as_ref() {
            disabled_tools.extend(profile.excluded_tools.iter().cloned());
        }
        if let Some(invocation_disabled_tools) = invocation_disabled_tools {
            disabled_tools.extend(invocation_disabled_tools.iter().cloned());
        }
        let tool_manager = Arc::clone(&agent.tools.tool_manager);
        if let Some(config) = agent.llm_config() {
            disabled_tools.extend(tool_manager.incompatible_tool_names(&config.input_modalities));
        }
        let mut skill_allowed_tools = agent.tools.skill_registry.active_skill_allowed_tools();
        let mut active_skill_names = agent.tools.skill_registry.activated_names();
        if let Some(progressive) = agent
            .tools
            .progressive_skill_registry
            .as_ref()
            .and_then(|registry| registry.try_read().ok())
        {
            if let Some(progressive_allowed) = progressive.active_skill_allowed_tools() {
                skill_allowed_tools
                    .get_or_insert_with(std::collections::HashSet::new)
                    .extend(progressive_allowed);
            }
            active_skill_names.extend(progressive.activated_names());
        }
        active_skill_names.sort();
        active_skill_names.dedup();
        let plan_mode = agent.config.plan_mode;
        let visibility = invocation_visible_tools.map(|initial| {
            let available = tool_manager
                .get_openai_tools()
                .into_iter()
                .filter(|tool| !disabled_tools.contains(&tool.function.name))
                .filter(|tool| {
                    !plan_mode
                        || (!crate::tools::is_write_tool(&tool.function.name)
                            && tool.function.name != "shell"
                            && tool.function.name != "delete_file")
                })
                .map(|tool| tool.function.name)
                .collect::<std::collections::HashSet<_>>();
            let eligible = available
                .iter()
                .filter(|name| {
                    skill_allowed_tools.as_ref().is_none_or(|allowed_tools| {
                        echo_execution::skills::external::types::skill_allows_tool(
                            allowed_tools,
                            name,
                        )
                    })
                })
                .cloned()
                .collect();
            let mut initial = initial.clone();
            initial.insert("tool_search".to_string());
            std::sync::Arc::new(echo_core::tools::ToolVisibilityState::with_available(
                available, eligible, initial,
            ))
        });
        Self {
            tool_manager,
            hook_registry: agent.tools.hook_registry.clone(),
            intervention_callbacks: agent.tools.intervention_callbacks.clone(),
            skill_allowed_tools,
            active_skill_names,
            plan_state: Arc::clone(&agent.plan_state),
            disabled_tools,
            visibility,
            plan_mode,
        }
    }

    /// Return the immutable, invocation-scoped tool definitions for the LLM.
    pub fn tools_for_llm(&self) -> Vec<crate::llm::types::ToolDefinition> {
        self.tool_manager
            .get_openai_tools()
            .into_iter()
            .filter(|tool| !self.disabled_tools.contains(&tool.function.name))
            .filter(|tool| self.visibility.is_some() || tool.function.name != "tool_search")
            .filter(|tool| {
                self.visibility
                    .as_ref()
                    .is_none_or(|visibility| visibility.is_visible(&tool.function.name))
            })
            .filter(|tool| self.is_skill_tool_allowed(&tool.function.name))
            .filter(|tool| {
                !self.plan_mode
                    || (!crate::tools::is_write_tool(&tool.function.name)
                        && tool.function.name != "shell"
                        && tool.function.name != "delete_file")
            })
            .collect()
    }

    pub(crate) fn is_skill_tool_allowed(&self, tool_name: &str) -> bool {
        if let Some(visibility) = self.visibility.as_ref() {
            return visibility.is_eligible(tool_name);
        }
        self.skill_allowed_tools
            .as_ref()
            .is_none_or(|allowed_tools| {
                echo_execution::skills::external::types::skill_allows_tool(allowed_tools, tool_name)
            })
    }
}

// ── GuardRuntime ─────────────────────────────────────────────────────

/// Guard / safety state. Shared via `Arc`.
#[derive(Clone)]
pub struct GuardRuntime {
    pub guard_manager: Option<Arc<crate::guard::GuardManager>>,
    pub audit_logger: Option<Arc<dyn AuditLogger>>,
    pub circuit_breaker: Option<Arc<CircuitBreaker>>,
}

impl GuardRuntime {
    pub fn from_agent(agent: &super::ReactAgent) -> Self {
        Self {
            guard_manager: agent.guard.guard_manager.clone().map(Arc::new),
            audit_logger: agent.guard.audit_logger.clone(),
            circuit_breaker: agent.guard.circuit_breaker.clone(),
        }
    }
}

// ── AgentRunSnapshot ─────────────────────────────────────────────────

/// Captures everything the streaming loop needs from a [`super::ReactAgent`] without
/// holding a reference to the agent itself.
///
/// Uses composition via `Arc` for all subsystems — cloning is O(1).
#[derive(Clone)]
pub struct AgentRunSnapshot {
    /// Immutable runtime configuration.
    pub config: Arc<RuntimeConfig>,
    /// Authoritative conversation context shared with the running agent.
    pub context: Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
    /// Tool execution state (tools, hooks).
    pub tools: Arc<ToolRuntime>,
    /// Guard / safety state.
    pub guard: Arc<GuardRuntime>,
    /// Snapshot manager (from memory subsystem).
    pub snapshot_manager: Arc<std::sync::RwLock<Option<SnapshotManager>>>,
    transcript_generation_id: Option<String>,
    transcript_projection_cursor: Arc<tokio::sync::Mutex<TranscriptProjectionCursor>>,
    /// HTTP client.
    pub client: Arc<reqwest::Client>,
    /// Optional trait-level LLM client. When present, the streaming core loop
    /// (`create_llm_stream`) and `direct_answer_stream` route LLM calls through
    /// this trait object instead of the raw `client` + model-resolve path —
    /// enabling test doubles (MockLlmClient) to drive the full ReAct loop.
    /// Production agents inject a real `LlmClient` implementation; execution
    /// returns a configuration error when none is attached.
    pub llm_client: Option<Arc<dyn crate::llm::LlmClient>>,
    /// Per-agent thinking-depth config, propagated to the think phase and react
    /// loop so each LLM request carries the configured reasoning depth. `None`
    /// means "use the model's default" (no thinking field sent).
    pub thinking: Option<crate::llm::ThinkingConfig>,
    /// Cancellation token (set after construction).
    pub cancel_token: Option<crate::agent::CancellationToken>,
    /// Shared same-turn input mailbox.
    pub(crate) turn_steer_mailbox: Arc<crate::agent::steer::TurnSteerMailbox>,
    /// Recently read files for read-before-edit enforcement (path → read instant).
    pub recently_read_files:
        Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    /// Run store for trace persistence.
    pub run_store: Option<Arc<dyn RunStore>>,
    /// Current run ID.
    pub current_run_id: Option<String>,
    /// Unique trace invocation ID. This is intentionally distinct from the
    /// product/business run ID in `current_run_id`.
    pub trace_run_id: Option<String>,
    /// Current user-input/agent turn ID.
    pub current_turn_id: Option<String>,
    /// Private authority for draining the exact active turn incarnation.
    pub(crate) turn_steer_incarnation: Option<Arc<()>>,
    /// Message that triggered the current invocation.
    pub current_message_id: Option<String>,
    /// Typed active user message, including any attachments.
    pub current_message: Option<crate::llm::types::Message>,
    /// Current concrete subagent/tool execution ID.
    pub current_execution_id: Option<String>,
    /// 外部 run 级上下文（跨 spawn 安全，从 ReactAgent.external_* 抓取）。
    /// 与 current_run_id 同源、同生命周期（set/clear 在同一处）。
    pub external_cancel: Option<std::sync::Arc<tokio_util::sync::CancellationToken>>,
    pub external_trace_sink: Option<echo_core::tools::TraceSinkFn>,
    pub external_delegation_policy: Option<echo_core::tools::NestedDelegationPolicy>,
    /// Opaque ownership tokens retained for this invocation and its tools.
    pub resource_guards: Vec<echo_core::tools::InvocationResourceGuard>,
    /// Permission service (human-in-the-loop).
    #[cfg(feature = "human-loop")]
    pub permission_service: Option<Arc<crate::human_loop::PermissionService>>,
    /// Approval rules registered through the synchronous agent setup API.
    #[cfg(feature = "human-loop")]
    pub pending_permission_rules:
        Arc<tokio::sync::Mutex<Vec<echo_core::tools::permission::PermissionRule>>>,
    /// Token usage tracker shared with the parent ReactAgent.
    pub token_tracker: Arc<echo_core::tokenizer::TokenUsageTracker>,
    /// Self-calibrating tokenizer shared with the parent ReactAgent. The think
    /// phase feeds real `usage.prompt_tokens` back into it so context-window
    /// and compression estimates converge to the model's actual tokenization.
    pub calibrated_tokenizer: Arc<echo_core::tokenizer::CalibratedTokenizer>,
    /// Runtime state store for rich checkpointing (messages + plan + skills).
    pub state_store: Option<Arc<dyn crate::state::RuntimeStateStore>>,
    /// Conversation store for user-visible transcript projection. When both
    /// this and `config.conversation_id` are set, the run loop persists
    /// projected messages at every finalization point so application UI history is
    /// always in sync with the running context — without each entry point
    /// having to re-implement the save logic.
    pub conversation_store: Option<Arc<dyn crate::memory::ConversationStore>>,
    /// Optional Critic for final_answer verification.
    pub critic: Option<Arc<dyn echo_core::agent::Critic>>,
    /// Optional tool execution pipeline (15-stage middleware).
    pub tool_execution_pipeline:
        Option<Arc<crate::agent::react::run::pipeline::ToolExecutionPipeline>>,
    /// (stage4 E1) Layered memory manager — used by `pre_compaction_flush` to
    /// write durable facts before compression. Cloned from the parent ReactAgent.
    pub memory_layer_manager: Option<Arc<crate::evolution::MemoryLayerManager>>,
    /// Optional application projection refreshed at the pre-prepare boundary.
    pub pre_model_context_projector: Option<Arc<dyn crate::compression::PreModelContextProjector>>,
    /// Consumer-supplied skill lifecycle curator used by telemetry writes.
    pub skill_curator: Option<crate::evolution::Curator>,
}

impl AgentRunSnapshot {
    /// Create a snapshot from a [`super::ReactAgent`].
    pub fn from_agent(agent: &super::ReactAgent) -> Self {
        let legacy = agent.capture_legacy_external_context();
        Self::from_agent_source(agent, None, Some(&legacy))
    }

    pub(crate) fn from_agent_with_legacy_context(
        agent: &super::ReactAgent,
        legacy: &crate::agent::react::LegacyExternalContextSnapshot,
    ) -> Self {
        Self::from_agent_source(agent, None, Some(legacy))
    }

    /// Create a snapshot whose run-scoped fields come from one invocation value.
    pub fn from_agent_with_invocation(
        agent: &super::ReactAgent,
        invocation: &echo_core::agent::AgentInvocationContext,
    ) -> Self {
        Self::from_agent_source(agent, Some(invocation), None)
    }

    fn from_agent_source(
        agent: &super::ReactAgent,
        invocation: Option<&echo_core::agent::AgentInvocationContext>,
        legacy: Option<&crate::agent::react::LegacyExternalContextSnapshot>,
    ) -> Self {
        let mut config = RuntimeConfig::from_agent_config(&agent.config);
        config.input_modalities = agent
            .llm_config()
            .map(|llm_config| llm_config.input_modalities.clone());
        if let Some(working_dir) = invocation.and_then(|context| context.working_dir.as_ref()) {
            config.working_dir = Some(working_dir.clone());
        }
        if let Some(run_budget) = invocation.and_then(|context| context.run_budget.as_ref()) {
            config.run_budget = run_budget.clone();
        }
        let runtime = invocation.and_then(|context| context.runtime.as_ref());
        if let Some(conversation_id) = runtime.and_then(|context| context.conversation_id.clone()) {
            config.conversation_id = Some(conversation_id.clone());
            config.runtime_state_id = Some(conversation_id);
        } else if invocation.is_none()
            && let Some(conversation_id) =
                legacy.and_then(|context| context.conversation_id.clone())
        {
            config.conversation_id = Some(conversation_id.clone());
            config.runtime_state_id = Some(conversation_id);
        }
        if let Some(runtime_state_id) =
            invocation.and_then(|context| context.runtime_state_id.clone())
        {
            config.runtime_state_id = Some(runtime_state_id);
        }
        Self {
            config: Arc::new(config),
            context: agent.memory.context.clone(),
            tools: Arc::new(ToolRuntime::from_agent(
                agent,
                invocation.and_then(|context| context.disabled_tools.as_ref()),
                invocation.and_then(|context| context.visible_tools.as_ref()),
            )),
            guard: Arc::new(GuardRuntime::from_agent(agent)),
            snapshot_manager: agent.memory.snapshot_manager.clone(),
            transcript_generation_id: invocation
                .and_then(|context| context.transcript_generation_id.clone()),
            transcript_projection_cursor: Arc::clone(&agent.memory.transcript_projection_cursor),
            client: agent.client().clone(),
            llm_client: agent.llm_client().cloned(),
            thinking: agent.thinking().cloned(),
            cancel_token: invocation.and_then(|context| {
                context.cancel.clone().or_else(|| {
                    runtime
                        .and_then(|value| value.cancel.as_ref())
                        .map(|cancel| cancel.as_ref().clone())
                })
            }),
            turn_steer_mailbox: Arc::clone(&agent.turn_steer_mailbox),
            recently_read_files: Arc::clone(&agent.recently_read_files),
            run_store: agent.run_store.clone(),
            current_run_id: if invocation.is_some() {
                runtime.and_then(|context| context.run_id.clone())
            } else {
                legacy.and_then(|context| context.current_run_id.clone())
            },
            trace_run_id: if invocation.is_some() {
                None
            } else {
                agent.capture_current_trace_run_id()
            },
            current_turn_id: if invocation.is_some() {
                runtime.and_then(|context| context.turn_id.clone())
            } else {
                legacy.and_then(|context| context.turn_id.clone())
            },
            turn_steer_incarnation: None,
            current_message_id: if invocation.is_some() {
                runtime.and_then(|context| context.message_id.clone())
            } else {
                legacy.and_then(|context| context.message_id.clone())
            },
            current_message: None,
            current_execution_id: if invocation.is_some() {
                runtime.and_then(|context| context.execution_id.clone())
            } else {
                legacy.and_then(|context| context.execution_id.clone())
            },
            external_cancel: if let Some(context) = invocation {
                runtime
                    .and_then(|value| value.cancel.clone())
                    .or_else(|| context.cancel.clone().map(Arc::new))
            } else {
                legacy.and_then(|context| context.cancel.clone())
            },
            external_trace_sink: if invocation.is_some() {
                runtime.and_then(|context| context.trace_sink.clone())
            } else {
                legacy.and_then(|context| context.trace_sink.clone())
            },
            external_delegation_policy: if invocation.is_some() {
                runtime.and_then(|context| context.delegation_policy)
            } else {
                legacy.and_then(|context| context.delegation_policy)
            },
            resource_guards: if let Some(context) = invocation {
                let mut guards = runtime
                    .map(|runtime| runtime.resource_guards.clone())
                    .unwrap_or_default();
                guards.extend(context.resource_guards.iter().cloned());
                guards
            } else {
                legacy
                    .map(|context| context.resource_guards.clone())
                    .unwrap_or_default()
            },
            #[cfg(feature = "human-loop")]
            permission_service: agent.approval.permission_service.clone(),
            #[cfg(feature = "human-loop")]
            pending_permission_rules: Arc::new(tokio::sync::Mutex::new(
                agent
                    .approval
                    .pending_permission_rules
                    .lock()
                    .map(|mut rules| std::mem::take(&mut *rules))
                    .unwrap_or_default(),
            )),
            token_tracker: Arc::clone(&agent.token_tracker),
            calibrated_tokenizer: Arc::clone(&agent.calibrated_tokenizer),
            state_store: agent.memory.state_store.clone(),
            conversation_store: agent.memory.conversation_store.clone(),
            critic: agent.critic.clone(),
            tool_execution_pipeline: agent.tool_execution_pipeline.clone(),
            memory_layer_manager: agent.memory_layer_manager.clone(),
            pre_model_context_projector: agent
                .pre_model_context_projector
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
            skill_curator: agent.skill_curator.clone(),
        }
    }

    // ── Trace helpers ──────────────────────────────────────────────

    /// Record a trace event if a run store is attached.
    pub async fn record_event(&self, event: RunEvent) {
        if let Some(ref store) = self.run_store
            && let Some(ref run_id) = self.trace_run_id
        {
            let _ = store.append_event(run_id, event).await;
        }
    }

    /// Finalize the current trace run (completed or failed).
    pub async fn finalize_run(&self, status: RunStatus, output: Option<&str>, error: Option<&str>) {
        if let Some(ref store) = self.run_store
            && let Some(ref run_id) = self.trace_run_id
            && let Ok(Some(mut run)) = store.load(run_id).await
        {
            run.status = status;
            run.final_output = output.map(|s| s.to_string());
            run.error = error.map(|s| s.to_string());
            run.finished_at = Some(chrono::Utc::now());
            let _ = store.save(run).await;
        }
    }

    /// Fire the aggregate lifecycle hook from the canonical tool-batch owner.
    pub(crate) async fn fire_post_tool_batch(
        &self,
        tool_names: &[String],
        success_count: usize,
        failure_count: usize,
    ) {
        let context = crate::skills::hooks::HookContext::for_post_tool_batch(
            tool_names,
            success_count,
            failure_count,
            self.config.session_id.as_deref().unwrap_or(""),
            &self.config.agent_name,
        );
        let registry = self.tools.hook_registry.read().await.clone();
        let _ = registry.run_lifecycle_hooks(&context).await;
    }

    // ── Runtime state checkpoint ─────────────────────────────────────

    /// Save a rich checkpoint to the [`RuntimeStateStore`](crate::state::RuntimeStateStore).
    ///
    /// Persists the full [`crate::state::AgentCheckpoint`] (messages, active skills, current
    /// plan, and blocked reason) so an in-flight conversation can resume
    /// across process restarts.
    ///
    /// Silently no-ops if no state store or runtime-state identity is configured.
    pub async fn save_runtime_checkpoint(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        blocked_reason: Option<String>,
    ) -> crate::error::Result<()> {
        let Some(ref store) = self.state_store else {
            return Ok(());
        };
        let Some(ref conv_id) = self.config.runtime_state_id else {
            return Ok(());
        };

        let messages = {
            let ctx = context.lock().await;
            ctx.messages().to_vec()
        };

        let transcript_projection = match self.transcript_generation_id.as_deref() {
            Some(generation_id) => Some(
                self.transcript_projection_cursor
                    .lock()
                    .await
                    .checkpoint_for(generation_id),
            ),
            None => None,
        };
        let messages_json = crate::state::AgentCheckpoint::serialize_payload(
            messages.clone(),
            transcript_projection,
        )?;

        let current_plan = self.tools.plan_state.read().await.clone();

        let checkpoint = crate::state::AgentCheckpoint {
            conversation_id: conv_id.clone(),
            messages_json,
            current_plan,
            active_skills: self.tools.active_skill_names.clone(),
            blocked_reason,
            working_dir: self.config.working_dir.clone(),
            timestamp: chrono::Utc::now(),
        };

        store.save_checkpoint(&checkpoint).await?;
        self.record_event(crate::trace::RunEvent::Checkpoint {
            id: format!(
                "checkpoint:{}:{}",
                conv_id,
                checkpoint.timestamp.timestamp_millis()
            ),
        })
        .await;
        tracing::debug!(
            conversation_id = conv_id.as_str(),
            message_count = messages.len(),
            "Runtime checkpoint saved"
        );
        Ok(())
    }

    /// Save the user-visible transcript projection to the [`ConversationStore`](crate::memory::ConversationStore).
    ///
    /// Unlike `save_runtime_checkpoint` which serializes the full runtime
    /// `Message` list (including internal/tool hand-offs) for resume, this
    /// projects messages to [`StoredMessage`](crate::memory::StoredMessage) records and persists
    /// them via `ConversationStore::save_messages` — the same shape that
    /// the application UI history panes consume.
    ///
    /// This consolidates transcript persistence in the framework: previously,
    /// every product entry point (Tauri commands, terminal UI loop) had to call
    /// `save_messages` on its own. Now `run_core_loop` invokes this helper at
    /// pre-model and finalization safe points, and the product layer only
    /// handles conversation metadata (title / pinned / agent_type).
    ///
    /// Silently no-ops if no conversation store or `conversation_id` is configured.
    pub async fn save_transcript_projection(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
    ) {
        let Some(ref store) = self.conversation_store else {
            return;
        };
        let Some(ref conv_id) = self.config.conversation_id else {
            return;
        };

        let messages = {
            let ctx = context.lock().await;
            filter_user_visible_transcript(ctx.messages())
        };

        if messages.is_empty() {
            tracing::debug!(
                conversation_id = conv_id.as_str(),
                "Transcript projection skipped: no user-visible messages"
            );
            return;
        }

        let projected = match crate::memory::project_messages(conv_id, &messages) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    conversation_id = conv_id.as_str(),
                    "Failed to project messages for conversation store"
                );
                return;
            }
        };

        // Ensure the conversation row exists. The default trait impl is a
        // get-or-create, so this is cheap on the hot path.
        let ensure = store
            .ensure_conversation(crate::memory::NewConversation {
                conversation_id: conv_id.clone(),
                user_id: "default".to_string(),
                agent_type: None,
                title: None,
            })
            .await;
        if let Err(e) = ensure {
            tracing::warn!(
                error = %e,
                conversation_id = conv_id.as_str(),
                "Failed to ensure conversation row before save_messages"
            );
            return;
        }

        // Generation-local cursor ownership serializes load/append/save safe
        // points for one Agent incarnation. Without it, two projections could
        // both load the same durable prefix and overwrite each other's suffix.
        let mut generation_cursor = match self.transcript_generation_id.as_deref() {
            Some(generation_id) => {
                let mut cursor = self.transcript_projection_cursor.lock().await;
                if cursor.generation_id.as_deref() != Some(generation_id) {
                    cursor.generation_id = Some(generation_id.to_string());
                    cursor.next_ordinal = 0;
                    cursor.projected.clear();
                }
                Some(cursor)
            }
            None => None,
        };
        let persisted = match store.get_messages(conv_id).await {
            Ok(messages) => messages,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    conversation_id = conv_id.as_str(),
                    "Failed to load durable transcript before merging active projection"
                );
                return;
            }
        };
        let mut assigned_projection = None;
        let mut cursor_before_assignment = None;
        let merged = if let (Some(cursor), Some(generation_id)) = (
            generation_cursor.as_mut(),
            self.transcript_generation_id.as_deref(),
        ) {
            cursor_before_assignment = Some((**cursor).clone());
            let assigned = match cursor.assign(generation_id, &projected) {
                Ok(assigned) => assigned,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        conversation_id = conv_id.as_str(),
                        "Failed to assign transcript generation ordinals"
                    );
                    return;
                }
            };
            let merged = match merge_generation_projection(
                persisted,
                &projected,
                &assigned,
                generation_id,
            ) {
                Ok(merged) => merged,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        generation_id,
                        "Failed to merge transcript generation projection"
                    );
                    if let Some(before) = cursor_before_assignment.take() {
                        **cursor = before;
                    }
                    return;
                }
            };
            assigned_projection = Some(assigned);
            merged
        } else {
            merge_transcript_projection(persisted, projected.clone())
        };

        if let Err(e) = store.save_messages(conv_id, &merged).await {
            if let (Some(cursor), Some(before)) =
                (generation_cursor.as_mut(), cursor_before_assignment.take())
            {
                **cursor = before;
            }
            tracing::warn!(
                error = %e,
                conversation_id = conv_id.as_str(),
                "Failed to save transcript projection to conversation store"
            );
        } else {
            if let (Some(cursor), Some(assigned)) =
                (generation_cursor.as_mut(), assigned_projection)
            {
                cursor.projected = assigned;
            }
            tracing::debug!(
                conversation_id = conv_id.as_str(),
                message_count = merged.len(),
                "Transcript projection saved"
            );
        }
    }

    /// Realign the generation cursor after compaction has replaced the active
    /// context. The complete pre-compaction transcript has already been saved,
    /// so retained messages keep their prior ordinals and only later messages
    /// receive new ordinals.
    pub(crate) async fn realign_transcript_projection(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
    ) -> crate::error::Result<()> {
        let Some(generation_id) = self.transcript_generation_id.as_deref() else {
            return Ok(());
        };
        let messages = {
            let context = context.lock().await;
            filter_user_visible_transcript(context.messages())
        };
        let conversation_id = self
            .config
            .conversation_id
            .as_deref()
            .unwrap_or(generation_id);
        let projected = crate::memory::project_messages(conversation_id, &messages)?;
        let mut cursor = self.transcript_projection_cursor.lock().await;
        let assigned = cursor.assign(generation_id, &projected)?;
        cursor.projected = assigned;
        Ok(())
    }

    // ── Tool execution helpers (delegated from Pipeline stages) ─────

    /// Check tool approval via PermissionService.
    /// Returns modified input if approval modified the tool call, None otherwise.
    #[cfg(feature = "human-loop")]
    pub async fn check_tool_approval(
        &self,
        request_id: &str,
        tool_name: &str,
        input: &serde_json::Value,
        permission_mode_override: Option<echo_core::tools::permission::PermissionMode>,
    ) -> std::result::Result<Option<serde_json::Value>, echo_core::error::ReactError> {
        if let Some(ref service) = self.permission_service {
            let pending = {
                let mut rules = self.pending_permission_rules.lock().await;
                std::mem::take(&mut *rules)
            };
            if !pending.is_empty() {
                service.add_rules(pending).await;
            }
            let permissions = self
                .tools
                .tool_manager
                .get_tool(tool_name)
                .map(|tool| tool.permissions())
                .unwrap_or_default();
            let classifier_context = {
                let messages = self.context.lock().await.messages().to_vec();
                let recent_files = self
                    .recently_read_files
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .keys()
                    .cloned()
                    .collect();
                let mut context = echo_orchestration::human_loop::ClassifierContext::new()
                    .with_messages(messages)
                    .with_recent_files(recent_files)
                    .with_risk_context(echo_orchestration::human_loop::RiskContext {
                        has_sensitive_files: false,
                        is_destructive: permissions.iter().any(|permission| {
                            matches!(
                                permission,
                                echo_core::tools::permission::ToolPermission::Write
                                    | echo_core::tools::permission::ToolPermission::Execute
                                    | echo_core::tools::permission::ToolPermission::Sensitive
                            )
                        }),
                        directory_depth: self
                            .config
                            .working_dir
                            .as_ref()
                            .map_or(0, |path| path.components().count()),
                        repetition_count: 0,
                    });
                if let Some(working_dir) = self.config.working_dir.as_ref() {
                    context = context.with_workspace_path(working_dir.display().to_string());
                }
                context
            };
            let permission_scope_id = self
                .config
                .conversation_id
                .as_deref()
                .or(self.config.session_id.as_deref())
                .map(|session| format!("{}:{}", self.config.agent_name, session));
            let permission_context = echo_orchestration::human_loop::PermissionInvocationContext {
                scope_id: permission_scope_id,
                request_id: Some(request_id.to_string()),
                session_id: self
                    .config
                    .conversation_id
                    .clone()
                    .or_else(|| self.config.session_id.clone()),
                agent_name: Some(self.config.agent_name.clone()),
                timeout: None,
                permission: echo_orchestration::human_loop::PermissionContext {
                    working_directory: self
                        .config
                        .working_dir
                        .as_ref()
                        .map(|path| path.display().to_string()),
                    affected_files: classifier_context.recent_files.clone(),
                    estimated_impact: None,
                    metadata: serde_json::Map::new(),
                },
                classifier: classifier_context,
            };
            let check = service.check_with_permissions_result_in_mode_and_context(
                tool_name,
                input,
                &permissions,
                permission_mode_override,
                Some(&permission_context),
            );
            tokio::pin!(check);
            let decision = if let Some(cancel) = self.cancel_token.as_ref() {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        return Err(echo_core::error::ReactError::Agent(Box::new(
                            echo_core::error::AgentError::Cancelled(format!(
                                "permission request for tool '{tool_name}'"
                            )),
                        )));
                    }
                    decision = &mut check => decision?,
                }
            } else {
                check.await?
            };
            match decision.decision {
                echo_core::tools::permission::PermissionDecision::Allow => {
                    Ok(decision.updated_input)
                }
                echo_core::tools::permission::PermissionDecision::Deny { reason } => {
                    Err(echo_core::error::ReactError::Other(format!(
                        "Permission denied for tool '{}': {}",
                        tool_name, reason
                    )))
                }
                echo_core::tools::permission::PermissionDecision::RequireApproval => {
                    Err(echo_core::error::ReactError::Other(format!(
                        "Tool '{}' requires user approval",
                        tool_name
                    )))
                }
                echo_core::tools::permission::PermissionDecision::Ask { suggestions } => {
                    Err(echo_core::error::ReactError::Other(format!(
                        "Tool '{}' requires user approval. Suggestions: {:?}",
                        tool_name, suggestions
                    )))
                }
            }
        } else {
            Ok(None)
        }
    }

    /// Check tool approval via PermissionService (no-op when human-loop feature is disabled).
    #[cfg(not(feature = "human-loop"))]
    pub async fn check_tool_approval(
        &self,
        _request_id: &str,
        _tool_name: &str,
        _input: &serde_json::Value,
        _permission_mode_override: Option<echo_core::tools::permission::PermissionMode>,
    ) -> std::result::Result<Option<serde_json::Value>, echo_core::error::ReactError> {
        Ok(None)
    }

    /// Record a file read for read-before-edit enforcement.
    pub fn record_file_read(&self, path: &str) {
        let path = std::path::Path::new(path);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(working_dir) = self.config.working_dir.as_deref() {
            working_dir.join(path)
        } else {
            path.to_path_buf()
        };
        let canonical = std::fs::canonicalize(&resolved)
            .unwrap_or(resolved)
            .to_string_lossy()
            .to_string();
        let mut files = self
            .recently_read_files
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        files.insert(canonical, std::time::Instant::now());
    }

    /// Check tool output guard and return filtered output if modified.
    pub async fn check_tool_output_guard(&self, output: &str) -> Option<String> {
        let mut effective_output = output.to_string();
        if crate::security::contains_secrets(&effective_output) {
            effective_output = crate::security::redact_secrets(&effective_output);
            tracing::warn!(agent = %self.config.agent_name, "Secret detected in tool output; redacted");
        }
        let Some(gm) = self.guard.guard_manager.as_ref() else {
            return (effective_output != output).then_some(effective_output);
        };
        use crate::guard::GuardDirection;
        let result = match gm
            .check_all(&effective_output, GuardDirection::Output)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(agent = %self.config.agent_name, error = %e, "Guard check failed, blocking output (fail-closed)");
                return Some(format!("Output content blocked: guard check error ({e})"));
            }
        };
        match result {
            crate::guard::GuardResult::Block { reason } => {
                tracing::info!(agent = %self.config.agent_name, reason = %reason, "🛡️ Tool output blocked by guard");
                if let Some(al) = &self.guard.audit_logger {
                    let event = crate::audit::AuditEvent::now(
                        self.config.session_id.clone(),
                        self.config.agent_name.clone(),
                        crate::audit::AuditEventType::GuardBlock {
                            guard: "guard_manager".to_string(),
                            direction: GuardDirection::Output,
                            reason: reason.clone(),
                        },
                    );
                    if let Err(e) = al.log(event).await {
                        tracing::error!(error = %e, "audit log write failed — event dropped");
                    }
                }
                Some(format!("Output content filtered by safety guard: {reason}"))
            }
            crate::guard::GuardResult::Transform { content, .. } => Some(content),
            crate::guard::GuardResult::Pass | crate::guard::GuardResult::Warn { .. } => {
                (effective_output != output).then_some(effective_output)
            }
        }
    }

    /// Apply the single authoritative spill/truncation policy for tool output.
    pub(crate) fn process_tool_output(&self, output: String) -> ProcessedToolOutput {
        self.process_tool_output_for_call(output, "unscoped", "tool", None)
    }

    pub(crate) fn process_tool_output_for_call(
        &self,
        output: String,
        call_id: &str,
        tool_name: &str,
        existing_artifact: Option<echo_core::tools::artifact::ToolOutputArtifactRef>,
    ) -> ProcessedToolOutput {
        let inline_bytes = output.len();
        let estimated_tokens = echo_core::tokenizer::HeuristicTokenizer.count_tokens(&output);
        let mut spill_error = None;
        let artifact = existing_artifact.or_else(|| {
            let mut config = self.config.tool_output_artifacts.clone()?;
            let exceeds_token_budget = self
                .config
                .max_tool_output_tokens
                .is_some_and(|max_tokens| estimated_tokens > max_tokens);
            if inline_bytes < config.threshold_bytes && !exceeds_token_budget {
                return None;
            }
            if exceeds_token_budget {
                config.threshold_bytes = 1;
            }
            let identity = echo_core::tools::artifact::ToolOutputArtifactIdentity {
                conversation_id: self.config.conversation_id.clone(),
                run_id: self
                    .current_run_id
                    .clone()
                    .or_else(|| self.current_turn_id.clone()),
                call_id: call_id.to_string(),
                tool_name: tool_name.to_string(),
            };
            match echo_core::tools::artifact::persist_tool_output(config, identity, &output) {
                Ok(artifact) => artifact,
                Err(error) => {
                    tracing::warn!(error = %error, "tool output artifact write failed; falling back to token truncation");
                    spill_error = Some(error.to_string());
                    None
                }
            }
        });

        if let Some(artifact) = artifact {
            let preview: String = output.chars().take(TOOL_OUTPUT_PREVIEW_CHARS).collect();
            let model_output = format!(
                "{preview}\n\n[Tool output preview only: the text above is not a summary and is not the complete result. Full output artifact: {} ({:.1} MiB, sha256 {}). Use read_artifact with this exact path and expected_sha256 to retrieve bounded pages until complete.]",
                artifact.path.display(),
                artifact.payload_bytes as f64 / 1_048_576.0,
                artifact.sha256,
            );
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("output_handling".to_string(), "spilled".to_string());
            metadata.insert(
                "original_bytes".to_string(),
                artifact.payload_bytes.to_string(),
            );
            metadata.insert("returned_bytes".to_string(), model_output.len().to_string());
            metadata.insert("estimated_tokens".to_string(), estimated_tokens.to_string());
            artifact.extend_metadata(&mut metadata);
            return ProcessedToolOutput {
                output: model_output,
                truncated: true,
                metadata,
            };
        }

        let max_tokens = self.config.max_tool_output_tokens.or_else(|| {
            spill_error
                .as_ref()
                .map(|_| TOOL_OUTPUT_SPILL_FAILURE_FALLBACK_TOKENS)
        });
        let Some(max_tokens) = max_tokens else {
            let estimated_tokens = echo_core::tokenizer::HeuristicTokenizer.count_tokens(&output);
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("output_handling".to_string(), "inline".to_string());
            metadata.insert("original_bytes".to_string(), inline_bytes.to_string());
            metadata.insert("returned_bytes".to_string(), inline_bytes.to_string());
            metadata.insert("estimated_tokens".to_string(), estimated_tokens.to_string());
            return ProcessedToolOutput {
                output,
                truncated: false,
                metadata,
            };
        };

        let tokenizer = echo_core::tokenizer::HeuristicTokenizer;
        let estimated_tokens = tokenizer.count_tokens(&output);
        if estimated_tokens <= max_tokens {
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("output_handling".to_string(), "inline".to_string());
            metadata.insert("original_bytes".to_string(), inline_bytes.to_string());
            metadata.insert("returned_bytes".to_string(), inline_bytes.to_string());
            metadata.insert("estimated_tokens".to_string(), estimated_tokens.to_string());
            return ProcessedToolOutput {
                output,
                truncated: false,
                metadata,
            };
        }

        let notice = format!(
            "\n\n[Output truncated: ~{estimated_tokens} tokens total, {max_tokens} token budget]\n\n"
        );
        let notice_tokens = tokenizer.count_tokens(&notice);
        let available_tokens = max_tokens.saturating_sub(notice_tokens);
        let available_chars = available_tokens.saturating_mul(4);
        let head_chars = available_chars.saturating_mul(7) / 10;
        let tail_chars = available_chars.saturating_sub(head_chars);
        let head: String = output.chars().take(head_chars).collect();
        let tail_reversed: String = output.chars().rev().take(tail_chars).collect();
        let tail: String = tail_reversed.chars().rev().collect();
        let truncated_output = if available_tokens == 0 {
            format!("[Output truncated: ~{estimated_tokens} tokens total]")
        } else {
            format!("{head}{notice}{tail}")
        };
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            "output_handling".to_string(),
            if spill_error.is_some() {
                "spill_failed_truncated"
            } else {
                "truncated"
            }
            .to_string(),
        );
        metadata.insert("original_bytes".to_string(), inline_bytes.to_string());
        metadata.insert(
            "returned_bytes".to_string(),
            truncated_output.len().to_string(),
        );
        metadata.insert("estimated_tokens".to_string(), estimated_tokens.to_string());
        if let Some(error) = spill_error {
            metadata.insert("spill_error".to_string(), error);
        }
        ProcessedToolOutput {
            output: truncated_output,
            truncated: true,
            metadata,
        }
    }

    /// Backward-compatible string view used by legacy internal call sites.
    pub async fn truncate_tool_output(&self, output: String) -> String {
        self.process_tool_output(output).output
    }

    // ── Lifecycle hook fan-out ───────────────────────────────────────

    /// Fire a lifecycle hook (`SessionEnd` / `PreCompact` / `StopFailure`).
    /// Used by the prepare / compact / finalize / max-iterations phases.
    pub(crate) async fn fire_hook(
        &self,
        event: crate::skills::hooks::HookEvent,
        matcher: Option<&str>,
    ) {
        let sid = self.config.session_id.clone().unwrap_or_default();
        let hc = match event {
            crate::skills::hooks::HookEvent::SessionEnd => {
                crate::skills::hooks::HookContext::for_session_end(
                    matcher.unwrap_or("other"),
                    &sid,
                    &self.config.agent_name,
                )
            }
            crate::skills::hooks::HookEvent::PreCompact => {
                crate::skills::hooks::HookContext::for_pre_compact(
                    &Default::default(),
                    matcher.unwrap_or("auto"),
                    &sid,
                    &self.config.agent_name,
                )
            }
            crate::skills::hooks::HookEvent::StopFailure => {
                crate::skills::hooks::HookContext::for_stop_failure(
                    "",
                    matcher.unwrap_or(""),
                    &sid,
                    &self.config.agent_name,
                )
            }
            _ => return,
        };
        let reg = self.tools.hook_registry.read().await.clone();
        let _ = reg.run_lifecycle_hooks(&hc).await;
    }

    // ── Auto snapshot (memory snapshot capture) ──────────────────────

    /// Capture a memory snapshot for the current iteration if the snapshot
    /// manager indicates one is due.
    pub(crate) async fn auto_snapshot(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        iteration: usize,
    ) {
        let should_capture = {
            let mgr = self
                .snapshot_manager
                .read()
                .unwrap_or_else(|e| e.into_inner());
            mgr.as_ref().is_some_and(|m| m.should_capture(iteration))
            // RwLockReadGuard dropped here — before any await
        };
        if should_capture {
            let ctx = context.lock().await;
            let ms = ctx.messages().to_vec();
            drop(ctx);
            if let Some(ref mut m) = *self
                .snapshot_manager
                .write()
                .unwrap_or_else(|e| e.into_inner())
            {
                m.capture(iteration, &ms);
            }
        }
    }

    // ── Tool approval (snapshot semantics) ───────────────────────────

    /// Whether a tool requires human approval before execution. This mirrors
    /// the streaming-path semantics: it consults `self.permission_service`
    /// directly without flushing pending permission rules (the non-streaming
    /// `ReactAgent::tool_needs_approval` in `run/approval.rs` does flush —
    /// this divergence is preserved intentionally to keep streaming behavior
    /// byte-identical to the pre-refactor implementation).
    #[cfg(feature = "human-loop")]
    pub(crate) async fn tool_needs_approval(&self, tool_name: &str) -> bool {
        use crate::tools::permission::PermissionMode;
        if let Some(svc) = &self.permission_service {
            let mode = svc.mode().await;
            if matches!(
                mode,
                PermissionMode::BypassPermissions | PermissionMode::DontAsk | PermissionMode::Plan
            ) {
                return false;
            }
            let perms = self
                .tools
                .tool_manager
                .get_tool(tool_name)
                .map(|t| t.permissions())
                .unwrap_or_default();
            return svc
                .would_request_human_for_permissions(tool_name, &perms)
                .await;
        }
        false
    }

    /// `human-loop` feature stub — no approval ever required.
    #[cfg(not(feature = "human-loop"))]
    #[allow(dead_code)]
    pub(crate) async fn tool_needs_approval(&self, _: &str) -> bool {
        false
    }

    // ── Tool execution (full pipeline) ───────────────────────────────

    /// Execute a single tool call with the full policy pipeline:
    /// PreToolUse hooks → read-before-edit guard → execute → PostToolUse hooks → audit.
    ///
    /// Uses the unified ToolExecutionPipeline (15 stages) for consistent behavior
    /// between streaming and non-streaming paths.
    pub(crate) fn execute_tool_with_policy<'a>(
        &'a self,
        call_id: String,
        tool_name: &'a str,
        params: &'a crate::tools::ToolParameters,
        input: &'a serde_json::Value,
        stream_tx: Option<
            tokio::sync::mpsc::Sender<crate::agent::react::run::pipeline::ToolPipelineEvent>,
        >,
    ) -> futures::future::BoxFuture<'a, std::result::Result<ToolCallSuccess, ToolCallFailure>> {
        Box::pin(async move {
            // Use the unified pipeline for consistent behavior
            let pipeline = self
                .tool_execution_pipeline
                .as_ref()
                .cloned()
                .unwrap_or_else(|| {
                    std::sync::Arc::new(
                        crate::agent::react::run::pipeline::ToolExecutionPipeline::default_pipeline(
                        ),
                    )
                });

            let mut ctx = crate::agent::react::run::pipeline::ToolExecutionContext {
                call_id,
                requested_tool_name: tool_name.to_string(),
                requested_input: input.clone(),
                tool_name: tool_name.to_string(),
                params: params.clone(),
                input: input.clone(),
                hook_messages: crate::agent::react::run::context::HookMessageBatches::default(),
                result: None,
                output: None,
                blocked: false,
                block_reason: None,
                block_failure: None,
                duration_ms: 0,
                plan_mode: self.config.plan_mode,
                permission_decision: None,
                permission_mode_override: None,
                rewrites: Vec::new(),
                invocation_emitted: false,
                stream_tx,
            };

            let pipeline_result = pipeline.run(&mut ctx, self).await;
            if !ctx.hook_messages.pre.is_empty() || !ctx.hook_messages.post.is_empty() {
                let mut context = self.context.lock().await;
                for message in &ctx.hook_messages.pre {
                    context.push(crate::agent::react::run::context::runtime_context_note(
                        "Hook:PreToolUse",
                        message,
                    ));
                }
                for message in &ctx.hook_messages.post {
                    context.push(crate::agent::react::run::context::runtime_context_note(
                        "Hook:PostToolUse",
                        message,
                    ));
                }
            }

            match pipeline_result {
                Ok(()) => {
                    // Check if execution was blocked
                    if ctx.blocked {
                        let reason = ctx
                            .block_reason
                            .unwrap_or_else(|| format!("Tool {} blocked", tool_name));
                        let failure = ctx.block_failure.unwrap_or_else(|| {
                            ToolFailure::new(crate::tools::ToolFailureCategory::Permanent)
                        });
                        let result = ToolResult::failure(failure.category, reason.clone())
                            .with_failure(failure);
                        return Err(ToolCallFailure {
                            name: ctx.tool_name.clone(),
                            error: crate::error::ToolError::ExecutionFailed {
                                tool: ctx.tool_name.clone(),
                                message: reason,
                            }
                            .into(),
                            result,
                        });
                    }

                    // Return the complete result after guard and output budgeting.
                    if let Some(mut result) = ctx.result {
                        if result.success {
                            if let Some(output) = ctx.output {
                                result.output = output;
                            }
                            return Ok(ToolCallSuccess {
                                name: ctx.tool_name,
                                result,
                            });
                        }
                        let message = result
                            .error
                            .clone()
                            .unwrap_or_else(|| result.output.clone());
                        let failure = result.failure.clone().unwrap_or_else(|| {
                            ToolFailure::new(crate::tools::ToolFailureCategory::Permanent)
                        });
                        result.error = Some(message.clone());
                        result.failure = Some(failure);
                        return Err(ToolCallFailure {
                            name: ctx.tool_name.clone(),
                            error: crate::error::ToolError::ExecutionFailed {
                                tool: ctx.tool_name.clone(),
                                message,
                            }
                            .into(),
                            result,
                        });
                    }
                    let message = "Pipeline completed without result".to_string();
                    Err(ToolCallFailure {
                        name: ctx.tool_name,
                        error: crate::error::ReactError::Other(message.clone()),
                        result: ToolResult::failure(
                            crate::tools::ToolFailureCategory::Permanent,
                            message,
                        ),
                    })
                }
                Err(error) => {
                    let may_have_side_effects = self
                        .tools
                        .tool_manager
                        .get_tool(tool_name)
                        .is_none_or(|tool| {
                            tool.risk_level() != crate::tools::ToolRiskLevel::ReadOnly
                        });
                    let failure = ToolFailure::from_error(&error, may_have_side_effects);
                    let result = ToolResult::failure(failure.category, error.to_string())
                        .with_failure(failure);
                    Err(ToolCallFailure {
                        name: ctx.tool_name,
                        result,
                        error,
                    })
                }
            }
        })
    }
}

#[cfg(test)]
mod transcript_filter_tests {
    use super::{
        AgentRunSnapshot, ToolRuntime, TranscriptProjectionCursor, filter_user_visible_transcript,
        merge_generation_projection,
    };
    use crate::compression::{ContextManager, ContextProjection};
    use crate::error::Result;
    use echo_core::llm::types::Message;
    use echo_core::tools::{Tool, ToolParameters, ToolResult};
    use std::collections::HashSet;
    use std::sync::Arc;

    struct NamedTool(&'static str);

    #[cfg(feature = "human-loop")]
    struct ApprovalTool;

    #[cfg(feature = "human-loop")]
    impl Tool for ApprovalTool {
        fn name(&self) -> &str {
            "approval_tool"
        }

        fn description(&self) -> &str {
            "tool that requires write approval"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn permissions(&self) -> Vec<echo_core::tools::permission::ToolPermission> {
            vec![echo_core::tools::permission::ToolPermission::Write]
        }

        fn execute<'a>(
            &'a self,
            _parameters: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async { Ok(ToolResult::success("ok")) })
        }
    }

    #[cfg(feature = "human-loop")]
    struct PendingApprovalProvider;

    #[cfg(feature = "human-loop")]
    impl crate::human_loop::HumanLoopProvider for PendingApprovalProvider {
        fn request(
            &self,
            _request: crate::human_loop::HumanLoopRequest,
        ) -> futures::future::BoxFuture<'_, Result<crate::human_loop::HumanLoopResponse>> {
            Box::pin(std::future::pending())
        }
    }

    impl Tool for NamedTool {
        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> &str {
            "snapshot policy test tool"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _parameters: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async { Ok(ToolResult::success("ok")) })
        }
    }

    #[test]
    fn invocation_separates_product_conversation_from_runtime_state_identity() {
        let config = crate::agent::AgentConfig::new("test-model", "agent", "system")
            .conversation_id("runtime-incarnation");
        let agent = crate::agent::ReactAgent::new(config);
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime_state_id: Some("runtime-incarnation".to_string()),
            runtime: Some(echo_core::tools::ExternalRunContext {
                conversation_id: Some("product-conversation".to_string()),
                run_id: None,
                turn_id: None,
                execution_id: None,
                isolation_id: None,
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
            }),
            ..Default::default()
        };

        let snapshot = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation);
        assert_eq!(
            snapshot.config.conversation_id.as_deref(),
            Some("product-conversation")
        );
        assert_eq!(
            snapshot.config.runtime_state_id.as_deref(),
            Some("runtime-incarnation")
        );
    }

    #[test]
    fn invocation_conversation_remains_checkpoint_identity_without_explicit_override() {
        let agent = crate::agent::ReactAgent::new(crate::agent::AgentConfig::new(
            "test-model",
            "agent",
            "system",
        ));
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime: Some(echo_core::tools::ExternalRunContext {
                conversation_id: Some("shared-identity".to_string()),
                run_id: None,
                turn_id: None,
                execution_id: None,
                isolation_id: None,
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
            }),
            ..Default::default()
        };

        let snapshot = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation);
        assert_eq!(
            snapshot.config.conversation_id,
            snapshot.config.runtime_state_id
        );
    }

    #[tokio::test]
    async fn fresh_runtime_context_appends_to_stable_product_transcript_without_model_restore()
    -> Result<()> {
        let old = crate::memory::project_messages(
            "product-conversation",
            &[
                Message::user("old product turn".to_string()),
                Message::assistant("same answer".to_string()),
            ],
        )?;
        let new = crate::memory::project_messages(
            "product-conversation",
            &[
                Message::user("again".to_string()),
                Message::assistant("same answer".to_string()),
            ],
        )?;
        let mut cursor = TranscriptProjectionCursor::default();
        let assigned = cursor.assign("new-runtime-incarnation", &new)?;
        let merged =
            merge_generation_projection(old.clone(), &new, &assigned, "new-runtime-incarnation")?;
        let text = merged
            .iter()
            .filter_map(|message| message.content.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            text,
            vec!["old product turn", "same answer", "again", "same answer"]
        );
        cursor.projected = assigned.clone();
        let same_safe_point = cursor.assign("new-runtime-incarnation", &new)?;
        assert_eq!(same_safe_point, assigned);
        let idempotent = merge_generation_projection(
            merged.clone(),
            &new,
            &same_safe_point,
            "new-runtime-incarnation",
        )?;
        assert_eq!(idempotent.len(), merged.len());

        let mut restored_before_product_save = TranscriptProjectionCursor::default();
        let checkpoint_before_product_save =
            restored_before_product_save.checkpoint_for("new-runtime-incarnation");
        restored_before_product_save.restore(checkpoint_before_product_save);
        let reassigned = restored_before_product_save.assign("new-runtime-incarnation", &new)?;
        let product_ahead = merge_generation_projection(
            merged.clone(),
            &new,
            &reassigned,
            "new-runtime-incarnation",
        )?;
        assert_eq!(product_ahead.len(), merged.len());
        let product_behind =
            merge_generation_projection(old, &new, &reassigned, "new-runtime-incarnation")?;
        assert_eq!(product_behind.len(), merged.len());

        let checkpoint = cursor.checkpoint_for("new-runtime-incarnation");
        let mut restored_cursor = TranscriptProjectionCursor::default();
        restored_cursor.restore(checkpoint);
        let compacted = crate::memory::project_messages(
            "product-conversation",
            &[
                Message::user("again".to_string()),
                Message::assistant("same answer".to_string()),
                Message::user("after compact".to_string()),
            ],
        )?;
        let after_compact = restored_cursor.assign("new-runtime-incarnation", &compacted)?;
        assert_eq!(after_compact.len(), 3);
        assert_eq!(after_compact.get(0).map(|message| message.ordinal), Some(0));
        assert_eq!(after_compact.get(1).map(|message| message.ordinal), Some(1));
        assert_eq!(after_compact.get(2).map(|message| message.ordinal), Some(2));

        let repeated = crate::memory::project_messages(
            "product-conversation",
            &[
                Message::user("A".to_string()),
                Message::assistant("B".to_string()),
                Message::user("A".to_string()),
                Message::assistant("B".to_string()),
            ],
        )?;
        let retained = crate::memory::project_messages(
            "product-conversation",
            &[
                Message::user("A".to_string()),
                Message::assistant("B".to_string()),
            ],
        )?;
        let mut repeated_cursor = TranscriptProjectionCursor::default();
        let repeated_projection = repeated_cursor.assign("repeat-generation", &repeated)?;
        repeated_cursor.projected = repeated_projection;
        let retained_projection = repeated_cursor.assign("repeat-generation", &retained)?;
        repeated_cursor.projected = retained_projection;
        let after_repeated_tail = repeated_cursor.assign("repeat-generation", &repeated)?;
        assert_eq!(
            after_repeated_tail
                .iter()
                .map(|message| message.ordinal)
                .collect::<Vec<_>>(),
            vec![2, 3, 4, 5]
        );

        let fresh_agent = crate::agent::ReactAgent::new(
            crate::agent::AgentConfig::new("test-model", "agent", "system")
                .conversation_id("new-runtime-incarnation"),
        );
        assert!(!fresh_agent.get_messages().await.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|text| text.contains("old product turn"))
        }));
        Ok(())
    }

    #[cfg(feature = "human-loop")]
    #[tokio::test]
    async fn permission_wait_stops_when_invocation_is_cancelled() -> Result<()> {
        let permission_service = Arc::new(crate::human_loop::PermissionService::from_provider(
            Arc::new(PendingApprovalProvider),
        ));
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .permission_service(permission_service)
            .tool(Box::new(ApprovalTool))
            .build()?;
        let cancel = crate::agent::CancellationToken::new();
        let invocation = echo_core::agent::AgentInvocationContext {
            cancel: Some(cancel.clone()),
            ..Default::default()
        };
        let snapshot = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation);
        let input = serde_json::json!({});
        let approval = snapshot.check_tool_approval("call-1", "approval_tool", &input, None);
        tokio::pin!(approval);

        tokio::task::yield_now().await;
        cancel.cancel();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), approval)
            .await
            .map_err(|_| {
                crate::error::ReactError::Other("approval cancellation timed out".into())
            })?;
        let error = match outcome {
            Ok(_) => {
                return Err(crate::error::ReactError::Other(
                    "cancelled permission wait unexpectedly approved the tool".into(),
                ));
            }
            Err(error) => error,
        };
        assert!(error.to_string().contains("permission request"));
        Ok(())
    }

    fn tool_names(snapshot: &AgentRunSnapshot) -> Vec<String> {
        snapshot
            .tools
            .tools_for_llm()
            .into_iter()
            .map(|tool| tool.function.name)
            .collect()
    }

    #[test]
    fn transcript_filter_excludes_projection_owned_messages_only() {
        let mut context = ContextManager::builder(4096).build();
        context.push(Message::user(
            "ordinary text mentions provider-marker".to_string(),
        ));
        context.apply_projections(&[ContextProjection {
            marker: "provider-marker".to_string(),
            message: Some(Message::user("internal projected state".to_string())),
        }]);
        context.push(Message::user("ordinary visible message".to_string()));

        let visible = filter_user_visible_transcript(context.messages());
        let visible_text: Vec<String> = visible
            .iter()
            .filter_map(|message| message.content.as_text())
            .collect();

        assert_eq!(
            visible_text,
            vec![
                "ordinary text mentions provider-marker".to_string(),
                "ordinary visible message".to_string(),
            ]
        );
    }

    #[test]
    fn invocation_snapshot_derives_runtime_fields_as_one_value() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .build()?;
        let cancel = std::sync::Arc::new(crate::agent::CancellationToken::new());
        let trace_sink: echo_core::tools::TraceSinkFn = std::sync::Arc::new(|_| {});
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime_state_id: None,
            transcript_generation_id: None,
            runtime: Some(echo_core::tools::ExternalRunContext {
                conversation_id: None,
                run_id: Some("run-atomic".to_string()),
                turn_id: None,
                execution_id: Some("execution-atomic".to_string()),
                isolation_id: None,
                message_id: Some("message-atomic".to_string()),
                cancel: Some(std::sync::Arc::clone(&cancel)),
                trace_sink: Some(std::sync::Arc::clone(&trace_sink)),
                delegation_policy: Some(echo_core::tools::NestedDelegationPolicy {
                    can_spawn_subagents: true,
                    delegate_depth: 3,
                    max_delegate_depth: 4,
                }),
                resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                    "runtime-guard".to_string(),
                )],
            }),
            working_dir: Some(std::path::PathBuf::from("/tmp/worktree-atomic")),
            cancel: None,
            disabled_tools: None,
            visible_tools: None,
            run_budget: None,
            history: None,
            resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                "invocation-guard".to_string(),
            )],
        };

        let snapshot = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation);
        assert_eq!(snapshot.current_run_id.as_deref(), Some("run-atomic"));
        assert_eq!(
            snapshot.config.working_dir.as_deref(),
            Some(std::path::Path::new("/tmp/worktree-atomic"))
        );
        assert!(
            snapshot
                .external_cancel
                .as_ref()
                .is_some_and(|value| std::sync::Arc::ptr_eq(value, &cancel))
        );
        assert!(
            snapshot
                .external_trace_sink
                .as_ref()
                .is_some_and(|value| std::sync::Arc::ptr_eq(value, &trace_sink))
        );
        assert_eq!(
            snapshot.external_delegation_policy,
            Some(echo_core::tools::NestedDelegationPolicy {
                can_spawn_subagents: true,
                delegate_depth: 3,
                max_delegate_depth: 4,
            })
        );
        assert_eq!(snapshot.resource_guards.len(), 2);
        Ok(())
    }

    #[test]
    fn invocation_tool_exclusions_are_isolated_and_snapshot_immutable() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .tool(Box::new(NamedTool("alpha")))
            .tool(Box::new(NamedTool("beta")))
            .tool(Box::new(NamedTool("gamma")))
            .tool(Box::new(NamedTool("delta")))
            .build()?;
        agent.set_disabled_tools(Some(HashSet::from(["gamma".to_string()])));

        let invocation_a = echo_core::agent::AgentInvocationContext {
            disabled_tools: Some(HashSet::from(["alpha".to_string()])),
            ..Default::default()
        };
        let invocation_b = echo_core::agent::AgentInvocationContext {
            disabled_tools: Some(HashSet::from(["beta".to_string()])),
            ..Default::default()
        };
        let snapshot_a = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation_a);
        let snapshot_b = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation_b);

        agent.set_disabled_tools(Some(HashSet::from(["delta".to_string()])));

        let tools_a: HashSet<String> = tool_names(&snapshot_a).into_iter().collect();
        let tools_b: HashSet<String> = tool_names(&snapshot_b).into_iter().collect();
        assert!(!tools_a.contains("alpha"));
        assert!(!tools_a.contains("gamma"));
        assert!(tools_a.contains("beta"));
        assert!(tools_a.contains("delta"));
        assert!(!tools_b.contains("beta"));
        assert!(!tools_b.contains("gamma"));
        assert!(tools_b.contains("alpha"));
        assert!(tools_b.contains("delta"));
        let snapshot_c = AgentRunSnapshot::from_agent(&agent);
        let tools_c: HashSet<String> = tool_names(&snapshot_c).into_iter().collect();
        assert!(tools_c.contains("alpha"));
        assert!(tools_c.contains("beta"));
        assert!(tools_c.contains("gamma"));
        assert!(!tools_c.contains("delta"));
        Ok(())
    }

    #[test]
    fn invocation_run_budget_overrides_agent_default_without_mutation() {
        let mut agent = crate::agent::ReactAgent::new(
            crate::agent::AgentConfig::new("model", "agent", "system").run_budget(
                echo_core::agent::RunBudgetPolicy {
                    iteration_wind_down_remaining: Some(2),
                    max_model_tokens: Some(1_000),
                },
            ),
        );
        let invocation = echo_core::agent::AgentInvocationContext {
            run_budget: Some(echo_core::agent::RunBudgetPolicy {
                iteration_wind_down_remaining: Some(1),
                max_model_tokens: Some(100),
            }),
            ..Default::default()
        };
        let snapshot = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation);
        agent.config_mut().run_budget.max_model_tokens = Some(5);

        assert_eq!(snapshot.config.run_budget.max_model_tokens, Some(100));
        assert_eq!(
            snapshot.config.run_budget.iteration_wind_down_remaining,
            Some(1)
        );
    }

    #[test]
    fn model_profile_exclusions_join_effective_tool_policy() -> Result<()> {
        let mut profile =
            echo_core::llm::capabilities::ModelProfile::from_provider_name("model", "openai");
        profile.excluded_tools.insert("shell".to_string());
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("model")
            .model_profile(profile)
            .tool(Box::new(NamedTool("shell")))
            .tool(Box::new(NamedTool("read_file")))
            .build()?;

        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let tools: HashSet<String> = tool_names(&snapshot).into_iter().collect();
        assert!(!tools.contains("shell"));
        assert!(tools.contains("read_file"));
        Ok(())
    }

    #[tokio::test]
    async fn model_profile_prompt_suffix_is_canonical_after_compression() -> Result<()> {
        let mut profile =
            echo_core::llm::capabilities::ModelProfile::from_provider_name("model", "openai");
        profile.prompt_suffix = Some("Use compact tool arguments.".to_string());
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("model")
            .system_prompt("Base prompt")
            .model_profile(profile)
            .token_limit(64)
            .build()?;

        let mut context = agent.memory.context.lock().await;
        context.push(echo_core::llm::types::Message::user(
            "temporary context".to_string(),
        ));
        let _ = context.force_compress(1).await?;
        assert!(context.messages().iter().any(|message| {
            message.text_content().is_some_and(|text| {
                text.contains("Base prompt") && text.contains("Use compact tool arguments.")
            })
        }));
        Ok(())
    }

    #[test]
    fn tool_visibility_combines_skill_plan_and_disabled_policies() {
        let manager = Arc::new(crate::tools::ToolManager::new());
        for name in ["read_file", "write_file", "shell", "final_answer", "custom"] {
            manager.register(Box::new(NamedTool(name)));
        }
        let runtime = ToolRuntime {
            tool_manager: manager,
            hook_registry: Arc::new(tokio::sync::RwLock::new(Default::default())),
            intervention_callbacks: Vec::new(),
            skill_allowed_tools: Some(HashSet::from([
                "read_file".to_string(),
                "write_file".to_string(),
                "shell".to_string(),
                "final_answer".to_string(),
            ])),
            active_skill_names: Vec::new(),
            plan_state: Arc::new(tokio::sync::RwLock::new(None)),
            disabled_tools: HashSet::from(["final_answer".to_string()]),
            visibility: None,
            plan_mode: true,
        };

        let visible: Vec<String> = runtime
            .tools_for_llm()
            .into_iter()
            .map(|tool| tool.function.name)
            .collect();

        assert_eq!(visible, vec!["read_file"]);
    }

    #[test]
    fn invocation_visibility_expands_without_mutating_registry() -> Result<()> {
        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .build()?;
        agent.add_tool(Box::new(NamedTool("custom")));
        let invocation = echo_core::agent::AgentInvocationContext {
            visible_tools: Some(HashSet::from([
                "final_answer".to_string(),
                "tool_search".to_string(),
            ])),
            ..Default::default()
        };
        let snapshot = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation);
        assert!(!tool_names(&snapshot).contains(&"custom".to_string()));

        let activated = snapshot
            .tools
            .visibility
            .as_ref()
            .map(|visibility| visibility.activate(["custom".to_string()]))
            .unwrap_or_default();
        assert_eq!(activated, vec!["custom"]);
        assert!(tool_names(&snapshot).contains(&"custom".to_string()));
        assert!(agent.tool_names().contains(&"custom".to_string()));
        Ok(())
    }

    #[test]
    fn tool_search_is_hidden_when_deferred_visibility_is_disabled() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);

        assert!(!tool_names(&snapshot).contains(&"tool_search".to_string()));
        assert!(agent.tool_names().contains(&"tool_search".to_string()));
        Ok(())
    }
}
