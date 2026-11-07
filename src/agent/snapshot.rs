//! Agent execution snapshot — captures agent state for `'static` streaming.
//!
//! [`AgentRunSnapshot`] replaces the old 33-field manual `AgentSnapshot` clone
//! in `stream_channel.rs` with a composition-based approach. Configuration,
//! tool runtime, and guard runtime are each wrapped in `Arc` so the snapshot
//! is cheap to clone and safe to move into a `tokio::spawn` future.

use crate::agent::AgentCallback;
use crate::agent::InterventionCallback;
use crate::audit::{
    AuditLogger, DiagnosticDeliveryFailure, DiagnosticDeliveryObserver,
    DiagnosticDeliveryOperation, DiagnosticRecordKind,
};
use crate::memory::snapshot::SnapshotManager;
use crate::skills::hooks::HookRegistry;
use crate::tools::{ToolExecutionConfig, ToolFailure, ToolManager, ToolResult};
use crate::trace::{RunEvent, RunStatus, RunStore};
use echo_core::circuit_breaker::CircuitBreaker;
use echo_core::llm::types::{Message, ResponseFormat, Role};
use echo_core::tokenizer::Tokenizer;
use std::sync::Arc;

const TOOL_OUTPUT_PREVIEW_CHARS: usize = 500;
const TOOL_OUTPUT_SPILL_FAILURE_FALLBACK_TOKENS: usize = 8_000;

/// Result of applying the run-scoped output budget to one tool result.
pub(crate) struct ProcessedToolOutput {
    pub output: String,
    pub truncated: bool,
    pub artifact: Option<echo_core::tools::artifact::ToolOutputArtifactRef>,
    pub metadata: std::collections::HashMap<String, String>,
}

pub(crate) struct ToolCallFailure {
    pub name: String,
    pub error: crate::error::ReactError,
    /// No result exists while the execution's physical settlement is unknown.
    pub result: Option<ToolResult>,
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

pub(crate) fn filter_user_visible_transcript(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter(|message| !is_internal_transcript_message(message))
        .cloned()
        .collect()
}

fn retained_context_bindings(
    previous: &[(String, Option<u64>)],
    current: &[crate::state::TranscriptProjectionMessage],
    compaction: bool,
) -> crate::error::Result<Vec<Option<Option<u64>>>> {
    let mut positions = Vec::new();
    if compaction {
        let old = previous
            .iter()
            .map(|(digest, _)| digest.as_str())
            .collect::<Vec<_>>();
        let new = current
            .iter()
            .map(|message| message.digest.as_str())
            .collect::<Vec<_>>();
        for change in similar::capture_diff_slices(similar::Algorithm::Lcs, &old, &new) {
            if matches!(change, similar::DiffOp::Equal { .. }) {
                positions.extend(change.new_range());
            }
        }
    } else {
        // Ordinary mutation only appends new facts after a retained prefix.
        let mut incoming = current.iter().enumerate();
        let mut next = incoming.next();
        for (digest, _) in previous {
            if next.is_some_and(|(_, message)| message.digest == *digest) {
                if let Some((position, _)) = next {
                    positions.push(position);
                }
                next = incoming.next();
            }
        }
    }
    // Matching the rightmost old embedding preserves repeated-tail identity;
    // it never deduplicates a genuinely appended same-text occurrence.
    let mut old = previous.iter().rev();
    let mut bindings = vec![None; current.len()];
    for position in positions.into_iter().rev() {
        let current = current.get(position).ok_or_else(|| {
            persistence_configuration_error("retained context position is missing")
        })?;
        let (_, ordinal) = old
            .by_ref()
            .find(|(digest, _)| *digest == current.digest)
            .ok_or_else(|| {
                persistence_configuration_error("retained context binding is missing")
            })?;
        let binding = bindings.get_mut(position).ok_or_else(|| {
            persistence_configuration_error("retained context output position is missing")
        })?;
        *binding = Some(*ordinal);
    }
    Ok(bindings)
}

fn transcript_projection_messages(
    projected: &[crate::memory::StoredMessage],
) -> crate::error::Result<Vec<crate::state::TranscriptProjectionMessage>> {
    projected
        .iter()
        .map(|message| {
            Ok(crate::state::TranscriptProjectionMessage {
                ordinal: 0,
                digest: crate::memory::transcript_projection_message_digest(message)?,
            })
        })
        .collect()
}

/// 沿现有context-only transform计算派生cursor，原source/cursor不改。
pub(crate) fn derive_context_view_checkpoint(
    original: &crate::state::ManagedRuntimeStateSnapshot,
    messages: Vec<Message>,
) -> crate::error::Result<crate::state::AgentCheckpoint> {
    let source = original
        .checkpoint
        .as_ref()
        .ok_or_else(|| persistence_configuration_error("checkpoint recovery source missing"))?;
    let payload = source.restore_managed_runtime_payload()?;
    if payload.pending_transcript_projection.is_some() {
        return Err(persistence_configuration_error(
            "checkpoint recovery cannot overwrite pending projection",
        ));
    }
    let cursor = if let Some(cursor) = payload.transcript_projection {
        let mut derived = TranscriptProjectionCursor::default();
        derived.restore(cursor);
        let projected = crate::memory::project_messages(
            &original.scope.scope_id,
            &filter_user_visible_transcript(&messages),
        )?;
        derived.assign_view(&original.runtime_state_id, &projected, true)?;
        Some(derived.checkpoint_for(&original.runtime_state_id))
    } else {
        None
    };
    let mut checkpoint = source.clone();
    checkpoint.messages_json = crate::state::AgentCheckpoint::serialize_payload(messages, cursor)?;
    checkpoint.restore_runtime_payload()?;
    Ok(checkpoint)
}

#[derive(Clone, Default)]
pub(crate) struct TranscriptProjectionCursor {
    generation_id: Option<String>,
    next_ordinal: u64,
    projected: Vec<crate::state::TranscriptProjectionMessage>,
    context_only: Option<Vec<crate::state::TranscriptContextOnlyMessage>>,
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
        self.context_only = None;
        self.generation_id = Some(generation_id.to_string());
        Ok(())
    }

    #[cfg(test)]
    fn assign(
        &mut self,
        generation_id: &str,
        projected: &[crate::memory::StoredMessage],
    ) -> crate::error::Result<Vec<crate::state::TranscriptProjectionMessage>> {
        Ok(self
            .assign_view(generation_id, projected, false)?
            .into_iter()
            .flatten()
            .collect())
    }

    fn assign_view(
        &mut self,
        generation_id: &str,
        projected: &[crate::memory::StoredMessage],
        compaction: bool,
    ) -> crate::error::Result<Vec<Option<crate::state::TranscriptProjectionMessage>>> {
        if self.generation_id.as_deref() != Some(generation_id) {
            self.generation_id = Some(generation_id.to_string());
            self.next_ordinal = 0;
            self.projected.clear();
            self.context_only = None;
        }
        let previous = self.checkpoint_for(generation_id).active_view()?;
        let identities = transcript_projection_messages(projected)?;
        let retained = retained_context_bindings(&previous, &identities, compaction)?;
        let mut assigned = Vec::with_capacity(identities.len());
        let mut derived = Vec::new();
        for (position, mut message) in identities.into_iter().enumerate() {
            let ordinal = match retained.get(position).copied().flatten() {
                Some(ordinal) => ordinal,
                None if compaction => None,
                None => {
                    let ordinal = self.next_ordinal;
                    self.next_ordinal = self.next_ordinal.checked_add(1).ok_or_else(|| {
                        crate::error::ReactError::Other(
                            "transcript projection ordinal capacity exhausted".to_string(),
                        )
                    })?;
                    Some(ordinal)
                }
            };
            if let Some(ordinal) = ordinal {
                message.ordinal = ordinal;
                assigned.push(Some(message));
            } else {
                derived.push(crate::state::TranscriptContextOnlyMessage {
                    position,
                    digest: message.digest,
                });
                assigned.push(None);
            }
        }
        self.projected = assigned.iter().flatten().cloned().collect();
        self.context_only = Some(derived);
        Ok(assigned)
    }

    fn checkpoint_for(&self, generation_id: &str) -> crate::state::TranscriptProjectionCheckpoint {
        crate::state::TranscriptProjectionCheckpoint {
            context_only: if self.generation_id.as_deref() == Some(generation_id) {
                self.context_only.clone()
            } else {
                None
            },
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
        self.context_only = checkpoint.context_only;
    }
}

/// 原read-map的来源观察；None是未取得source证明的诊断事实，不能授予写许可。
/// 仅进程内Rust数据，没有序列化或新wire字段。
pub type FileReadHistory = std::collections::HashMap<
    String,
    Vec<(
        Option<Arc<dyn echo_core::tools::filesystem::FileSystem>>,
        std::time::Instant,
    )>,
>;

pub(crate) fn same_file_tracking_source(
    left: &Arc<dyn echo_core::tools::filesystem::FileSystem>,
    right: &Arc<dyn echo_core::tools::filesystem::FileSystem>,
) -> bool {
    // paths_are_native明确声明本进程同一namespace；其它来源只认同一保留Arc。
    (left.paths_are_native() && right.paths_are_native()) || Arc::ptr_eq(left, right)
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
    /// Budget rebuilt from the same fresh model profile as `token_limit`.
    pub token_budget: Option<echo_core::budget::TokenBudget>,
    /// Construction-time validation failure for the configured token budget.
    pub token_budget_error: Option<String>,
    pub run_budget: echo_core::agent::RunBudgetPolicy,
    pub supports_tool_choice_none: bool,
    /// 模型HTTP模式，独立于公开调用返回String还是事件流；默认保持流式。
    pub model_streaming: bool,
    /// Caller-declared final response format, fixed for this run.
    pub response_format: Option<ResponseFormat>,
    /// Resolved provider/model capability, never inferred from the format request.
    pub supports_structured_output: bool,
    /// Model fact provenance applied to this immutable run snapshot.
    pub model_fact_sources: Vec<echo_core::llm::capabilities::ModelFactMetadata>,
    /// Expired or future-observed model facts rejected at resolution time.
    pub ignored_model_facts: Vec<echo_core::llm::capabilities::ModelFactMetadata>,
    /// Tokenizer fact recorded for diagnostics; runtime estimation remains the
    /// framework's calibrated heuristic tokenizer until tokenizer dispatch is
    /// implemented as a separate provider contract.
    pub resolved_tokenizer_name: Option<String>,
    /// Input modalities accepted by the configured model. `None` preserves
    /// compatibility for custom agents that do not provide an
    /// [`crate::llm::LlmConfig`].
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
    /// Total budget for one transcript persistence safe point.
    pub persistence_settlement_timeout: std::time::Duration,
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
        Self::from_agent_config_with_model_profile(config, None)
    }

    /// Create a snapshot using the fresh profile resolved for this invocation.
    ///
    /// The profile receipt is the authority for both the context limit and the
    /// token budget. Keeping those values together prevents a refreshed model
    /// fact from changing one policy while leaving the other on a stale value.
    pub fn from_agent_config_with_model_profile(
        config: &crate::agent::AgentConfig,
        model_profile: Option<&echo_core::llm::capabilities::ModelProfileResolution>,
    ) -> Self {
        let model_profile = model_profile.or(config.model_profile.as_ref());
        let token_limit = if config.token_limit_explicit {
            config.token_limit
        } else {
            model_profile
                .and_then(|resolution| resolution.profile.context_window)
                .and_then(|window| usize::try_from(window).ok())
                .map(|window| window.clamp(1, 10_000_000))
                .unwrap_or(crate::agent::config::DEFAULT_TOKEN_LIMIT)
        };
        let token_budget_result = if config.token_budget_config.enabled {
            let mut token_budget_config = config.token_budget_config.clone();
            token_budget_config.total_window = Some(token_limit);
            Some(token_budget_config.build(token_limit))
        } else {
            None
        };
        let max_tokens = config.max_tokens.or_else(|| {
            model_profile
                .and_then(|resolution| resolution.profile.max_output_tokens)
                .filter(|value| *value > 0)
        });
        Self {
            agent_name: config.agent_name.clone(),
            model_name: config.model_name.clone(),
            model_streaming: config.model_streaming,
            provider: model_profile.map(|resolution| resolution.profile.provider.clone()),
            max_iterations: config.max_iterations,
            token_limit,
            token_budget: token_budget_result
                .as_ref()
                .and_then(|result| result.as_ref().ok().cloned()),
            token_budget_error: token_budget_result
                .and_then(|result| result.err())
                .map(|error| error.to_string()),
            run_budget: config.run_budget.clone(),
            supports_tool_choice_none: model_profile
                .is_none_or(|resolution| resolution.profile.supports_tool_choice_none),
            response_format: config.response_format.clone(),
            supports_structured_output: model_profile
                .is_some_and(|resolution| resolution.profile.capabilities.structured_output),
            model_fact_sources: model_profile
                .map(|resolution| resolution.applied_facts.clone())
                .unwrap_or_default(),
            ignored_model_facts: model_profile
                .map(|resolution| resolution.ignored_stale_facts.clone())
                .unwrap_or_default(),
            resolved_tokenizer_name: model_profile
                .and_then(|resolution| resolution.resolved_tokenizer_name.clone()),
            input_modalities: None,
            session_id: config.session_id.clone(),
            runtime_state_id: config.conversation_id.clone(),
            conversation_id: config.conversation_id.clone(),
            working_dir: config.working_dir.lock().ok().and_then(|g| g.clone()),
            temperature: config.temperature,
            max_tokens,
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
            persistence_settlement_timeout: config.persistence_settlement_timeout,
            verifier_enabled: config.verifier_enabled,
            verifier_min_score: config.verifier_min_score,
            verifier_max_retries: config.verifier_max_retries,
            plan_mode: config.plan_mode
                || config.permission_mode == echo_core::tools::permission::PermissionMode::Plan,
            cache_user_id: config.cache_user_id.clone(),
        }
    }

    /// Return the session ID as a &str, defaulting to empty.
    pub fn session_id_str(&self) -> &str {
        self.session_id.as_deref().unwrap_or("")
    }
}

pub(crate) fn effective_runtime_state_id<'a>(
    configured_runtime_state_id: Option<&'a str>,
    invocation: Option<&'a echo_core::agent::AgentInvocationContext>,
) -> Option<&'a str> {
    invocation
        .and_then(|context| context.runtime_state_id.as_deref())
        .or_else(|| {
            invocation
                .and_then(|context| context.runtime.as_ref())
                .and_then(|runtime| runtime.conversation_id.as_deref())
        })
        .or(configured_runtime_state_id)
}

pub(crate) fn validate_transcript_generation_identity(
    runtime_state_id: Option<&str>,
    transcript_generation_id: Option<&str>,
) -> crate::error::Result<()> {
    if let (Some(runtime_state_id), Some(transcript_generation_id)) =
        (runtime_state_id, transcript_generation_id)
        && runtime_state_id != transcript_generation_id
    {
        return Err(crate::error::ReactError::RuntimeState(Box::new(
            echo_core::error::RuntimeStateError::SerializationError(format!(
                "transcript generation identity '{transcript_generation_id}' does not match runtime state identity '{runtime_state_id}'",
            )),
        )));
    }
    Ok(())
}

pub(crate) fn transcript_settlement_admission_error(
    settlement: &crate::memory::TranscriptProjectionSettlement,
) -> crate::error::ReactError {
    if settlement.status == crate::memory::TranscriptProjectionSettlementStatus::Deferred {
        return echo_core::error::RuntimeStateError::TranscriptProjectionDeferred {
            operation_id: settlement.operation_id.clone(),
            reason: settlement
                .detail
                .clone()
                .unwrap_or_else(|| "durable transcript debt remains unsettled".to_string()),
        }
        .into();
    }
    echo_core::error::RuntimeStateError::TranscriptProjectionBlocked {
        status: format!("{:?}", settlement.status),
        reason: settlement
            .detail
            .clone()
            .unwrap_or_else(|| "transcript projection admission failed".to_string()),
    }
    .into()
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
    /// Effective disabled tools captured for this invocation.
    pub disabled_tools: std::collections::HashSet<String>,
    /// Mutable schema visibility for deferred tools in this invocation.
    pub visibility: Option<std::sync::Arc<echo_core::tools::ToolVisibilityState>>,
    /// Whether the invocation uses plan mode's read-only tool surface.
    pub plan_mode: bool,
    /// 锁后快照唯一求值的有效只读策略，合并配置和本次调用上界。
    pub readonly_tools: bool,
    /// Live permission-mode authority for mode changes made through SDK/host APIs.
    #[cfg(feature = "human-loop")]
    permission_service: Option<Arc<crate::human_loop::PermissionService>>,
}

impl ToolRuntime {
    pub fn from_agent(
        agent: &super::ReactAgent,
        model_profile: Option<&echo_core::llm::capabilities::ModelProfileResolution>,
        invocation_disabled_tools: Option<&std::collections::HashSet<String>>,
        invocation_visible_tools: Option<&std::collections::HashSet<String>>,
    ) -> Self {
        Self::from_agent_with_tool_scope(
            agent,
            model_profile,
            invocation_disabled_tools,
            invocation_visible_tools,
            None,
            None,
            None,
        )
    }

    fn from_agent_with_tool_scope(
        agent: &super::ReactAgent,
        model_profile: Option<&echo_core::llm::capabilities::ModelProfileResolution>,
        invocation_disabled_tools: Option<&std::collections::HashSet<String>>,
        invocation_visible_tools: Option<&std::collections::HashSet<String>>,
        invocation_allowed_tools: Option<&std::collections::HashSet<String>>,
        invocation_tool_access: Option<echo_core::tools::ToolAccess>,
        delegation_policy: Option<echo_core::tools::NestedDelegationPolicy>,
    ) -> Self {
        let invocation_allowed_tools =
            invocation_allowed_tools.filter(|allowed| !allowed.is_empty());
        let mut disabled_tools = agent.tools.tool_visibility.disabled_names();
        let delegation_blocked = delegation_policy.is_some_and(|policy| !policy.can_delegate());
        if delegation_blocked {
            disabled_tools.insert("agent_tool".to_string());
        }
        if let Some(resolution) = model_profile.or(agent.config.model_profile.as_ref()) {
            disabled_tools.extend(resolution.profile.excluded_tools.iter().cloned());
        }
        if let Some(invocation_disabled_tools) = invocation_disabled_tools {
            disabled_tools.extend(invocation_disabled_tools.iter().cloned());
        }
        let tool_manager = Arc::clone(&agent.tools.tool_manager);
        if let Some(config) = agent.llm_config() {
            disabled_tools.extend(tool_manager.incompatible_tool_names(&config.input_modalities));
        }
        let readonly_tools = agent.config.readonly_tools
            || invocation_tool_access == Some(echo_core::tools::ToolAccess::ReadOnly);
        let skill_allowed_tools = agent.tools.skill_registry.active_skill_allowed_tools();
        #[cfg(feature = "human-loop")]
        let permission_service = agent.approval.permission_service.clone();
        let plan_mode = agent.config.plan_mode
            || agent.config.permission_mode == echo_core::tools::permission::PermissionMode::Plan
            || {
                #[cfg(feature = "human-loop")]
                {
                    permission_service.as_ref().is_some_and(|service| {
                        service.current_mode() == echo_core::tools::permission::PermissionMode::Plan
                    })
                }
                #[cfg(not(feature = "human-loop"))]
                {
                    false
                }
            };
        // The positive hard scope survives queueing. Resolve it against this
        // current catalog, independently of the initial schema visibility.
        // Explicit nondelegating policy needs the same available/eligible
        // authority even without deferred schemas. Tool search must not fall
        // back to the shared registry and rediscover the excluded dispatcher.
        let explicit_visibility =
            invocation_visible_tools.is_some() || invocation_allowed_tools.is_some();
        let visibility = (explicit_visibility || delegation_blocked || readonly_tools).then(|| {
            let current_tools = tool_manager.get_openai_tools();
            if let Some(allowed) = invocation_allowed_tools {
                // Keep the original unavailable diagnostic for names known at
                // admission. The positive bound below also covers later names.
                disabled_tools.extend(
                    current_tools
                        .iter()
                        .map(|tool| tool.function.name.clone())
                        .filter(|name| !allowed.contains(name)),
                );
            }
            let available = current_tools
                .into_iter()
                .filter(|tool| {
                    invocation_allowed_tools
                        .is_none_or(|allowed| allowed.contains(&tool.function.name))
                })
                .filter(|tool| !disabled_tools.contains(&tool.function.name))
                .filter(|tool| {
                    !(plan_mode || readonly_tools)
                        || tool_manager
                            .get_tool(&tool.function.name)
                            .is_some_and(|tool| tool.capabilities().is_read_only())
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
            let mut initial = invocation_visible_tools
                .cloned()
                .unwrap_or_else(|| available.clone());
            if explicit_visibility {
                initial.insert("tool_search".to_string());
            } else {
                // Preserve NoDeferred schemas while a forced search still
                // receives this qualified catalog instead of a fallback.
                initial.remove("tool_search");
            }
            std::sync::Arc::new(echo_core::tools::ToolVisibilityState::with_available(
                available, eligible, initial,
            ))
        });
        Self {
            tool_manager,
            hook_registry: agent.tools.hook_registry.clone(),
            intervention_callbacks: agent.tools.intervention_callbacks.clone(),
            skill_allowed_tools,
            disabled_tools,
            visibility,
            plan_mode,
            readonly_tools,
            #[cfg(feature = "human-loop")]
            permission_service,
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
                !(self.is_plan_mode() || self.readonly_tools)
                    || self
                        .tool_manager
                        .get_tool(&tool.function.name)
                        .is_some_and(|tool| tool.capabilities().is_read_only())
            })
            .collect()
    }

    pub(crate) fn is_tool_read_only(&self, tool_name: &str) -> bool {
        self.tool_manager
            .get_tool(tool_name)
            .is_some_and(|tool| tool.capabilities().is_read_only())
    }

    pub(crate) fn is_plan_mode(&self) -> bool {
        self.plan_mode || {
            #[cfg(feature = "human-loop")]
            {
                self.permission_service.as_ref().is_some_and(|service| {
                    service.current_mode() == echo_core::tools::permission::PermissionMode::Plan
                })
            }
            #[cfg(not(feature = "human-loop"))]
            {
                false
            }
        }
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
    /// Canonical activation authority shared with the owning agent and Skill tools.
    skill_activation: crate::skills::SkillActivationHandle,
    /// Guard / safety state.
    pub guard: Arc<GuardRuntime>,
    /// Snapshot manager (from memory subsystem).
    pub snapshot_manager: Arc<std::sync::RwLock<Option<SnapshotManager>>>,
    transcript_generation_id: Option<String>,
    transcript_projection_cursor: Arc<tokio::sync::Mutex<TranscriptProjectionCursor>>,
    runtime_state_version: Arc<tokio::sync::Mutex<Option<crate::state::RuntimeStateVersion>>>,
    runtime_state_hydration:
        Arc<tokio::sync::Mutex<crate::agent::react::subsystems::memory::RuntimeStateHydration>>,
    transcript_settlement_observed: Arc<std::sync::atomic::AtomicBool>,
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
    pub recently_read_files: Arc<std::sync::Mutex<FileReadHistory>>,
    /// Run store for trace persistence.
    pub run_store: Option<Arc<dyn RunStore>>,
    /// Observer for Trace and Audit persistence delivery failures.
    pub(crate) diagnostic_delivery_observer: Option<Arc<dyn DiagnosticDeliveryObserver>>,
    /// Current run ID.
    pub current_run_id: Option<String>,
    /// Unique trace invocation ID. This is intentionally distinct from the
    /// product/business run ID in `current_run_id`.
    pub trace_run_id: Option<String>,
    /// Guard前恢复只暂存本次恢复事实；trace准入后才发布，不借用上一轮trace。
    pub(crate) pending_checkpoint_resume: Option<crate::trace::RunEvent>,
    /// Invocation-local authority for calls that actually entered ExecuteStage.
    executing_tool_call_ids: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
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
    /// 调用值携带的run上下文；随同一快照跨spawn保留，绝不回读Agent共享setter。
    pub external_cancel: Option<std::sync::Arc<tokio_util::sync::CancellationToken>>,
    pub external_trace_sink: Option<echo_core::tools::TraceSinkFn>,
    pub external_delegation_policy: Option<echo_core::tools::NestedDelegationPolicy>,
    /// Identity/lineage of the dispatched Subagent attempt this snapshot runs.
    /// `None` for primary agent invocations. Copied into every ToolContext.
    pub subagent_lineage: Option<echo_core::tools::SubagentLineage>,
    /// Uplink channel for Subagent→parent / Subagent→sibling messaging,
    /// installed by the dispatcher into the invocation's runtime context.
    pub external_uplink: Option<echo_core::tools::SubagentUplinkFn>,
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
    /// Runtime state store for ReAct checkpointing (messages + skills).
    /// 计划状态由revisioned Task graph持有，运行时检查点不承载计划文本。
    pub state_store: Option<Arc<dyn crate::state::RuntimeStateStore>>,
    /// Conversation store for user-visible transcript projection. When both
    /// this and `config.conversation_id` are set, the run loop persists
    /// projected messages at every finalization point so application UI history is
    /// always in sync with the running context — without each entry point
    /// having to re-implement the save logic.
    pub conversation_store: Option<Arc<dyn crate::memory::ConversationStore>>,
    /// Long-term store used by the optional framework skill telemetry writer.
    pub(crate) memory_store: Option<Arc<dyn crate::memory::Store>>,
    /// Optional Critic for final_answer verification.
    pub critic: Option<Arc<dyn echo_core::agent::Critic>>,
    /// Optional tool execution pipeline (17-stage middleware).
    pub tool_execution_pipeline:
        Option<Arc<crate::agent::react::run::pipeline::ToolExecutionPipeline>>,
    /// (stage4 E1) Layered memory manager — used by `pre_compaction_flush` to
    /// write durable facts before compression. Cloned from the parent ReactAgent.
    pub memory_layer_manager: Option<Arc<crate::evolution::MemoryLayerManager>>,
    /// Optional application projection refreshed at the pre-prepare boundary.
    pub pre_model_context_projector: Option<Arc<dyn crate::compression::PreModelContextProjector>>,
    /// Consumer-supplied canonical Skill lifecycle mutation authority.
    pub skill_mutation_authority: Option<Arc<crate::evolution::SkillMutationAuthority>>,
}

struct AgentPersistenceCoordinator<'a> {
    snapshot: &'a AgentRunSnapshot,
    deadline: tokio::time::Instant,
    call_context: crate::memory::PersistenceCallContext,
}

/// Owns the volatile transform until its original checkpoint CAS accepts it.
/// Context is released before Store I/O so read-only Store callbacks can reenter.
pub(crate) struct PreparedContextCompaction<'a> {
    snapshot: &'a AgentRunSnapshot,
    context: Option<tokio::sync::MutexGuard<'a, crate::compression::ContextManager>>,
    cursor: tokio::sync::MutexGuard<'a, TranscriptProjectionCursor>,
    version: tokio::sync::MutexGuard<'a, Option<crate::state::RuntimeStateVersion>>,
    hydration:
        tokio::sync::MutexGuard<'a, crate::agent::react::subsystems::memory::RuntimeStateHydration>,
    original_context: crate::compression::PreparedContextTransform,
    original_messages: Vec<Message>,
    original_cursor: TranscriptProjectionCursor,
    original_hydration: crate::agent::react::subsystems::memory::RuntimeStateHydration,
    restore_context: bool,
    completed: bool,
}

impl PreparedContextCompaction<'_> {
    pub(crate) fn context_mut(
        &mut self,
    ) -> crate::error::Result<&mut crate::compression::ContextManager> {
        self.context
            .as_deref_mut()
            .ok_or_else(|| persistence_configuration_error("compaction context is staged"))
    }

    pub(crate) async fn commit(
        mut self,
        temporary: Option<&dyn crate::compression::ContextCompressor>,
    ) -> crate::error::Result<()> {
        let snapshot = self.snapshot;
        let messages = self.context_mut()?.messages().to_vec();
        let managed = snapshot.conversation_store.is_some();
        let generation = snapshot.transcript_generation_id.as_deref();
        let mut candidate_cursor = self.original_cursor.clone();
        if managed {
            let generation = generation
                .ok_or_else(|| persistence_configuration_error("missing transcript generation"))?;
            let scope =
                snapshot.config.conversation_id.as_deref().ok_or_else(|| {
                    persistence_configuration_error("missing conversation identity")
                })?;
            let projected =
                crate::memory::project_messages(scope, &filter_user_visible_transcript(&messages))?;
            candidate_cursor.assign_view(generation, &projected, true)?;
        }
        let binding_changed = generation.is_some_and(|generation| {
            candidate_cursor.checkpoint_for(generation)
                != self.original_cursor.checkpoint_for(generation)
        });
        let messages_changed =
            serde_json::to_value(&messages)? != serde_json::to_value(&self.original_messages)?;
        let original = self.original_context.clone();
        let candidate_context = self.context_mut()?.exchange_context_transform(original);
        self.restore_context = false;
        drop(self.context.take());

        let mut accepted_version = None;
        if managed && (messages_changed || binding_changed) {
            // Start the persistence deadline after calculation, not before a
            // potentially long summary-model request.
            let coordinator = AgentPersistenceCoordinator::new(snapshot)?;
            let (conversations, runtime) = coordinator.managed_stores().ok_or_else(|| {
                persistence_configuration_error("managed compaction stores missing")
            })?;
            let (scope, runtime_id, generation) = coordinator.identities()?;
            let epoch = coordinator
                .ensure_epoch(conversations.as_ref(), scope)
                .await?;
            if !matches!(
                epoch.status,
                crate::memory::ConversationProjectionEpochStatus::Created
                    | crate::memory::ConversationProjectionEpochStatus::AdoptedLegacy
                    | crate::memory::ConversationProjectionEpochStatus::Existing
                    | crate::memory::ConversationProjectionEpochStatus::Recreated
            ) {
                return Err(persistence_configuration_error(
                    "conversation projection epoch is fenced",
                ));
            }
            let (scope_revision, expected, current) = coordinator
                .current_runtime_state(runtime.as_ref(), scope, runtime_id)
                .await?;
            if self.version.as_ref()
                != Some(&AgentPersistenceCoordinator::authority_version(
                    current.as_ref(),
                ))
            {
                return Err(
                    echo_core::error::RuntimeStateError::ManagedStateRequiresCas(
                        "runtime authority changed during compaction".to_string(),
                    )
                    .into(),
                );
            }
            if current
                .as_ref()
                .and_then(|state| state.checkpoint.as_ref())
                .map(crate::state::AgentCheckpoint::restore_managed_runtime_payload)
                .transpose()?
                .is_some_and(|payload| payload.pending_transcript_projection.is_some())
            {
                return Err(
                    echo_core::error::RuntimeStateError::ManagedStateRequiresCas(
                        "compaction cannot overwrite pending transcript projection".to_string(),
                    )
                    .into(),
                );
            }
            let checkpoint = coordinator.build_frozen_checkpoint(
                messages,
                None,
                Some(candidate_cursor.checkpoint_for(generation)),
                None,
            )?;
            let receipt = coordinator
                .save_frozen_checkpoint(
                    runtime.as_ref(),
                    crate::state::RuntimeCheckpointCasRequest {
                        managed_import: None,
                        scope_id: scope.to_string(),
                        runtime_state_id: runtime_id.to_string(),
                        conversation_epoch: Some(epoch.authority.epoch),
                        expected_scope_revision: scope_revision,
                        expected_state_version: expected,
                        checkpoint,
                    },
                )
                .await?;
            accepted_version = Some(receipt.version);
        }
        self.context = Some(snapshot.context.lock().await);
        let original = self.original_context.clone();
        if !self.context_mut()?.matches_context_transform(&original)? {
            return Err(
                echo_core::error::RuntimeStateError::ManagedStateRequiresCas(
                    "public context changed while compaction checkpoint was settling".to_string(),
                )
                .into(),
            );
        }
        if managed {
            *self.cursor = candidate_cursor;
        }
        let persisted = accepted_version.is_some();
        if let Some(version) = accepted_version {
            *self.version = Some(version);
        }
        *self.hydration = self.original_hydration.clone();
        // No await after publication starts. Custom synchronous observers see
        // the accepted view, never a rejected summary candidate.
        self.completed = true;
        self.context_mut()?
            .accept_context_transform(candidate_context, temporary);
        drop(self);
        if persisted && let Some(runtime_id) = snapshot.config.runtime_state_id.as_deref() {
            snapshot.record_checkpoint_event(runtime_id).await;
        }
        Ok(())
    }
}

impl Drop for PreparedContextCompaction<'_> {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        if self.restore_context
            && let Some(context) = self.context.as_deref_mut()
        {
            context.exchange_context_transform(self.original_context.clone());
        }
        *self.cursor = self.original_cursor.clone();
        if self.snapshot.conversation_store.is_some() {
            *self.version = None;
            *self.hydration =
                crate::agent::react::subsystems::memory::RuntimeStateHydration::Hydrating(
                    self.snapshot.config.runtime_state_id.clone(),
                );
        }
        // Promotion may already have had an external effect. Recovery reloads
        // authority; it never calls the compressor/promoter to undo or replay it.
    }
}

type ManagedPersistenceStores<'a> = (
    &'a Arc<dyn crate::memory::ConversationStore>,
    &'a Arc<dyn crate::state::RuntimeStateStore>,
);

impl<'a> AgentPersistenceCoordinator<'a> {
    fn new(snapshot: &'a AgentRunSnapshot) -> crate::error::Result<Self> {
        let started_at = tokio::time::Instant::now();
        let timeout = snapshot.config.persistence_settlement_timeout;
        if timeout.is_zero() {
            return Err(persistence_configuration_error(
                "persistence settlement timeout must be greater than zero",
            ));
        }
        let deadline = started_at.checked_add(timeout).ok_or_else(|| {
            persistence_configuration_error("persistence settlement deadline overflow")
        })?;
        Ok(Self {
            snapshot,
            deadline,
            call_context: crate::memory::PersistenceCallContext::with_timeout(timeout)?,
        })
    }

    fn managed_stores(&self) -> Option<ManagedPersistenceStores<'_>> {
        Some((
            self.snapshot.conversation_store.as_ref()?,
            self.snapshot.state_store.as_ref()?,
        ))
    }

    fn identities(&self) -> crate::error::Result<(&str, &str, &str)> {
        let conversation_id = self
            .snapshot
            .config
            .conversation_id
            .as_deref()
            .ok_or_else(|| persistence_configuration_error("missing conversation identity"))?;
        let runtime_state_id = self
            .snapshot
            .config
            .runtime_state_id
            .as_deref()
            .ok_or_else(|| persistence_configuration_error("missing runtime-state identity"))?;
        let generation_id = self
            .snapshot
            .transcript_generation_id
            .as_deref()
            .ok_or_else(|| persistence_configuration_error("missing transcript generation"))?;
        validate_transcript_generation_identity(Some(runtime_state_id), Some(generation_id))?;
        Ok((conversation_id, runtime_state_id, generation_id))
    }

    async fn ensure_epoch(
        &self,
        store: &dyn crate::memory::ConversationStore,
        conversation_id: &str,
    ) -> crate::error::Result<crate::memory::ConversationProjectionEpochReceipt> {
        tokio::time::timeout_at(
            self.deadline,
            store.ensure_projection_epoch_with_context(
                self.call_context,
                crate::memory::EnsureConversationProjectionRequest {
                    conversation: crate::memory::NewConversation {
                        conversation_id: conversation_id.to_string(),
                        user_id: "default".to_string(),
                        agent_type: None,
                        title: None,
                    },
                    expected_tombstone_epoch: None,
                },
            ),
        )
        .await
        .map_err(|_| persistence_deadline_error("conversation epoch acquisition"))?
    }

    async fn current_runtime_state(
        &self,
        store: &dyn crate::state::RuntimeStateStore,
        scope_id: &str,
        runtime_state_id: &str,
    ) -> crate::error::Result<(
        u64,
        crate::state::RuntimeStateExpectedVersion,
        Option<crate::state::ManagedRuntimeStateSnapshot>,
    )> {
        self.current_runtime_state_observed(store, scope_id, runtime_state_id, false)
            .await
    }

    async fn current_runtime_state_observed(
        &self,
        store: &dyn crate::state::RuntimeStateStore,
        scope_id: &str,
        runtime_state_id: &str,
        await_physical_settlement: bool,
    ) -> crate::error::Result<(
        u64,
        crate::state::RuntimeStateExpectedVersion,
        Option<crate::state::ManagedRuntimeStateSnapshot>,
    )> {
        let scope_load = store.load_scope_authority_with_context(self.call_context, scope_id);
        let scope = if await_physical_settlement {
            scope_load.await?
        } else {
            tokio::time::timeout_at(self.deadline, scope_load)
                .await
                .map_err(|_| persistence_deadline_error("runtime scope load"))??
        };
        let scope_revision = scope.map(|scope| scope.revision).unwrap_or(0);
        let state_load =
            store.load_runtime_state_with_context(self.call_context, scope_id, runtime_state_id);
        let current = if await_physical_settlement {
            state_load.await?
        } else {
            tokio::time::timeout_at(self.deadline, state_load)
                .await
                .map_err(|_| persistence_deadline_error("runtime generation load"))??
        };
        let expected = match current.as_ref().map(|state| &state.version) {
            None | Some(crate::state::RuntimeStateVersion::Absent) => {
                crate::state::RuntimeStateExpectedVersion::Absent
            }
            Some(crate::state::RuntimeStateVersion::Unmanaged { digest }) => {
                crate::state::RuntimeStateExpectedVersion::Unmanaged {
                    digest: digest.clone(),
                }
            }
            Some(crate::state::RuntimeStateVersion::Managed { revision }) => {
                crate::state::RuntimeStateExpectedVersion::Managed {
                    revision: *revision,
                }
            }
            Some(crate::state::RuntimeStateVersion::Retired { .. }) => {
                return Err(crate::error::ReactError::RuntimeState(Box::new(
                    echo_core::error::RuntimeStateError::ManagedStateRequiresCas(format!(
                        "runtime generation {runtime_state_id} is retired"
                    )),
                )));
            }
        };
        Ok((scope_revision, expected, current))
    }

    async fn build_checkpoint(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        blocked_reason: Option<String>,
        cursor: Option<crate::state::TranscriptProjectionCheckpoint>,
        pending: Option<crate::state::PendingTranscriptProjection>,
    ) -> crate::error::Result<crate::state::AgentCheckpoint> {
        let messages = context.lock().await.messages().to_vec();
        self.build_frozen_checkpoint(messages, blocked_reason, cursor, pending)
    }

    fn build_frozen_checkpoint(
        &self,
        messages: Vec<Message>,
        blocked_reason: Option<String>,
        cursor: Option<crate::state::TranscriptProjectionCheckpoint>,
        pending: Option<crate::state::PendingTranscriptProjection>,
    ) -> crate::error::Result<crate::state::AgentCheckpoint> {
        let runtime_state_id = self
            .snapshot
            .config
            .runtime_state_id
            .as_ref()
            .ok_or_else(|| persistence_configuration_error("missing runtime-state identity"))?;
        let messages_json =
            crate::state::AgentCheckpoint::serialize_managed_payload(messages, cursor, pending)?;
        Ok(crate::state::AgentCheckpoint {
            conversation_id: runtime_state_id.clone(),
            messages_json,

            active_skills: self.snapshot.active_skill_names(),
            blocked_reason,
            working_dir: self.snapshot.config.working_dir.clone(),
            timestamp: chrono::Utc::now(),
        })
    }

    fn next_managed_revision(
        expected: &crate::state::RuntimeStateExpectedVersion,
    ) -> crate::error::Result<u64> {
        match expected {
            crate::state::RuntimeStateExpectedVersion::Absent
            | crate::state::RuntimeStateExpectedVersion::Unmanaged { .. } => Ok(1),
            crate::state::RuntimeStateExpectedVersion::Managed { revision } => {
                revision.checked_add(1).ok_or_else(|| {
                    echo_core::error::RuntimeStateError::RevisionExhausted(
                        "checkpoint revision reached u64::MAX".to_string(),
                    )
                    .into()
                })
            }
        }
    }

    fn authority_version(
        current: Option<&crate::state::ManagedRuntimeStateSnapshot>,
    ) -> crate::state::RuntimeStateVersion {
        current
            .map(|state| state.version.clone())
            .unwrap_or(crate::state::RuntimeStateVersion::Absent)
    }

    async fn require_hydrated_base(
        &self,
        current: Option<&crate::state::ManagedRuntimeStateSnapshot>,
    ) -> crate::error::Result<()> {
        if matches!(
            *self.snapshot.runtime_state_hydration.lock().await,
            crate::agent::react::subsystems::memory::RuntimeStateHydration::Hydrating(_)
        ) {
            return Err(
                echo_core::error::RuntimeStateError::ManagedStateRequiresCas(
                    "context transformation requires authoritative runtime hydration".to_string(),
                )
                .into(),
            );
        }
        let durable = Self::authority_version(current);
        let mut hydrated = self.snapshot.runtime_state_version.lock().await;
        match hydrated.as_ref() {
            Some(version) if version == &durable => Ok(()),
            None if durable == crate::state::RuntimeStateVersion::Absent => {
                *hydrated = Some(durable);
                Ok(())
            }
            _ => Err(
                echo_core::error::RuntimeStateError::ManagedStateRequiresCas(
                    "in-memory context was hydrated from a different runtime revision".to_string(),
                )
                .into(),
            ),
        }
    }

    async fn save_checkpoint(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        blocked_reason: Option<String>,
        prepared_cursor: Option<crate::state::TranscriptProjectionCheckpoint>,
    ) -> crate::error::Result<()> {
        let Some((conversation_store, runtime_store)) = self.managed_stores() else {
            return self.save_legacy_checkpoint(context, blocked_reason).await;
        };
        let (scope_id, runtime_state_id, generation_id) = self.identities()?;
        let epoch = self
            .ensure_epoch(conversation_store.as_ref(), scope_id)
            .await?;
        if !matches!(
            epoch.status,
            crate::memory::ConversationProjectionEpochStatus::Created
                | crate::memory::ConversationProjectionEpochStatus::AdoptedLegacy
                | crate::memory::ConversationProjectionEpochStatus::Existing
                | crate::memory::ConversationProjectionEpochStatus::Recreated
        ) {
            return Err(persistence_configuration_error(
                "conversation projection epoch is fenced",
            ));
        }
        let (scope_revision, expected, current) = self
            .current_runtime_state(runtime_store.as_ref(), scope_id, runtime_state_id)
            .await?;
        self.require_hydrated_base(current.as_ref()).await?;
        if current
            .as_ref()
            .and_then(|state| state.checkpoint.as_ref())
            .map(crate::state::AgentCheckpoint::restore_managed_runtime_payload)
            .transpose()?
            .is_some_and(|payload| payload.pending_transcript_projection.is_some())
        {
            return Err(crate::error::ReactError::RuntimeState(Box::new(
                echo_core::error::RuntimeStateError::ManagedStateRequiresCas(
                    "checkpoint-only save cannot overwrite pending transcript projection"
                        .to_string(),
                ),
            )));
        }
        let cursor = match prepared_cursor {
            Some(cursor) => cursor,
            None => self
                .snapshot
                .transcript_projection_cursor
                .lock()
                .await
                .checkpoint_for(generation_id),
        };
        let checkpoint = self
            .build_checkpoint(context, blocked_reason, Some(cursor.clone()), None)
            .await?;
        let mut in_memory_cursor = self.snapshot.transcript_projection_cursor.lock().await;
        let mut version = self.snapshot.runtime_state_version.lock().await;
        let receipt = self
            .save_frozen_checkpoint(
                runtime_store.as_ref(),
                crate::state::RuntimeCheckpointCasRequest {
                    managed_import: None,
                    scope_id: scope_id.to_string(),
                    runtime_state_id: runtime_state_id.to_string(),
                    conversation_epoch: Some(epoch.authority.epoch),
                    expected_scope_revision: scope_revision,
                    expected_state_version: expected,
                    checkpoint,
                },
            )
            .await?;
        in_memory_cursor.restore(cursor);
        *version = Some(receipt.version);
        drop(version);
        drop(in_memory_cursor);
        self.snapshot
            .record_checkpoint_event(runtime_state_id)
            .await;
        Ok(())
    }

    async fn save_frozen_checkpoint(
        &self,
        runtime_store: &dyn crate::state::RuntimeStateStore,
        request: crate::state::RuntimeCheckpointCasRequest,
    ) -> crate::error::Result<crate::state::RuntimeCheckpointCasReceipt> {
        self.save_frozen_checkpoint_observed(runtime_store, request, false)
            .await
    }

    async fn save_frozen_checkpoint_observed(
        &self,
        runtime_store: &dyn crate::state::RuntimeStateStore,
        request: crate::state::RuntimeCheckpointCasRequest,
        await_physical_settlement: bool,
    ) -> crate::error::Result<crate::state::RuntimeCheckpointCasReceipt> {
        let operation =
            runtime_store.compare_and_save_checkpoint_with_context(self.call_context, request);
        // 新runtime-only versioned路径须等原Store真实结算；同一deadline/receipt校验，
        // 不复制CAS策略，也不让调用侧timeout丢弃已准入blocking写。
        let receipt = if await_physical_settlement {
            operation.await?
        } else {
            tokio::time::timeout_at(self.deadline, operation)
                .await
                .map_err(|_| persistence_deadline_error("checkpoint compare-and-save"))??
        };
        if !matches!(
            receipt.status,
            crate::state::RuntimeCheckpointCasStatus::Applied
                | crate::state::RuntimeCheckpointCasStatus::AlreadyCurrent
        ) {
            return Err(
                echo_core::error::RuntimeStateError::ManagedStateRequiresCas(format!(
                    "checkpoint compare-and-save did not settle: {:?}",
                    receipt.status
                ))
                .into(),
            );
        }
        Ok(receipt)
    }

    async fn settle_transcript_projection(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        blocked_reason: Option<String>,
    ) -> crate::error::Result<crate::memory::TranscriptProjectionSettlement> {
        let Some((conversation_store, runtime_store)) = self.managed_stores() else {
            self.save_legacy_checkpoint(context, blocked_reason).await?;
            return Ok(settlement(
                crate::memory::TranscriptProjectionSettlementStatus::Settled,
                None,
                self.snapshot.config.conversation_id.clone(),
                self.snapshot.transcript_generation_id.clone(),
                0,
                None,
                None,
            ));
        };
        let (scope_id, runtime_state_id, generation_id) = self.identities()?;
        let epoch = self
            .ensure_epoch(conversation_store.as_ref(), scope_id)
            .await?;
        if !matches!(
            epoch.status,
            crate::memory::ConversationProjectionEpochStatus::Created
                | crate::memory::ConversationProjectionEpochStatus::AdoptedLegacy
                | crate::memory::ConversationProjectionEpochStatus::Existing
                | crate::memory::ConversationProjectionEpochStatus::Recreated
        ) {
            return Ok(settlement(
                crate::memory::TranscriptProjectionSettlementStatus::Conflict,
                None,
                Some(scope_id.to_string()),
                Some(generation_id.to_string()),
                0,
                Some(crate::memory::TranscriptProjectionErrorClass::SemanticConflict),
                Some("conversation epoch is fenced".to_string()),
            ));
        }

        let (scope_revision, expected, current) = self
            .current_runtime_state(runtime_store.as_ref(), scope_id, runtime_state_id)
            .await?;
        if let Some(state) = current.as_ref()
            && let Some(checkpoint) = state.checkpoint.as_ref()
        {
            let payload = checkpoint.restore_managed_runtime_payload()?;
            if payload.pending_transcript_projection.is_some() {
                let prior = crate::state::settle_loaded_pending_transcript_projection(
                    conversation_store.as_ref(),
                    runtime_store.as_ref(),
                    scope_id,
                    runtime_state_id,
                    state.clone(),
                    crate::state::TranscriptProjectionDispatch::RetryDurablePending,
                    self.call_context,
                )
                .await?
                .settlement;
                if prior.status != crate::memory::TranscriptProjectionSettlementStatus::Settled {
                    return Ok(prior);
                }
                *self.snapshot.runtime_state_version.lock().await = None;
                return Ok(settlement(
                    crate::memory::TranscriptProjectionSettlementStatus::Blocked,
                    None,
                    Some(scope_id.to_string()),
                    Some(generation_id.to_string()),
                    0,
                    Some(crate::memory::TranscriptProjectionErrorClass::RevisionConflict),
                    Some(
                        "runtime pending settled; rehydrate context before a new projection"
                            .to_string(),
                    ),
                ));
            }
        }
        self.require_hydrated_base(current.as_ref()).await?;

        let all_messages = context.lock().await.messages().to_vec();
        let visible_messages = filter_user_visible_transcript(&all_messages);
        let projected = crate::memory::project_messages(scope_id, &visible_messages)?;
        {
            let prepare_attempt = 1_u32;
            let cursor_before = match current.as_ref().and_then(|state| state.checkpoint.as_ref()) {
                Some(checkpoint) => checkpoint
                    .restore_managed_runtime_payload()?
                    .transcript_projection
                    .unwrap_or_else(|| crate::state::TranscriptProjectionCheckpoint {
                        context_only: None,
                        generation_id: generation_id.to_string(),
                        next_ordinal: 0,
                        projected: Vec::new(),
                    }),
                None => self
                    .snapshot
                    .transcript_projection_cursor
                    .lock()
                    .await
                    .checkpoint_for(generation_id),
            };
            let mut working_cursor = TranscriptProjectionCursor {
                generation_id: Some(cursor_before.generation_id.clone()),
                next_ordinal: cursor_before.next_ordinal,
                projected: cursor_before.projected.clone(),
                context_only: cursor_before.context_only.clone(),
            };
            let assigned = working_cursor.assign_view(generation_id, &projected, false)?;
            let cursor_after = working_cursor.checkpoint_for(generation_id);
            let mut new_messages = Vec::new();
            for (mut message, identity) in projected.iter().cloned().zip(assigned.iter()) {
                let Some(identity) = identity else {
                    continue;
                };
                if identity.ordinal < cursor_before.next_ordinal {
                    continue;
                }
                crate::memory::set_transcript_projection_meta(
                    &mut message,
                    generation_id,
                    identity.ordinal,
                )?;
                new_messages.push(message);
            }
            if new_messages.is_empty() {
                self.save_checkpoint(context, blocked_reason.clone(), Some(cursor_after))
                    .await?;
                return Ok(settlement(
                    crate::memory::TranscriptProjectionSettlementStatus::Settled,
                    None,
                    Some(scope_id.to_string()),
                    Some(generation_id.to_string()),
                    0,
                    None,
                    None,
                ));
            }

            let batch = crate::memory::TranscriptProjectionBatch::prepare(
                scope_id,
                epoch.authority.epoch,
                generation_id,
                cursor_before.next_ordinal,
                new_messages,
            )?;
            let pending = crate::state::PendingTranscriptProjection {
                batch: batch.clone(),
                cursor_before: cursor_before.clone(),
                cursor_after,
                base_runtime_revision: Self::next_managed_revision(&expected)?,
                prepared_at: chrono::Utc::now(),
                attempt: prepare_attempt,
                last_attempt_class: None,
                last_error: None,
            };
            let checkpoint = self
                .build_checkpoint(
                    context,
                    blocked_reason.clone(),
                    Some(cursor_before),
                    Some(pending.clone()),
                )
                .await?;
            let prepare_receipt = tokio::time::timeout_at(
                self.deadline,
                runtime_store.compare_and_save_checkpoint_with_context(
                    self.call_context,
                    crate::state::RuntimeCheckpointCasRequest {
                        managed_import: None,
                        scope_id: scope_id.to_string(),
                        runtime_state_id: runtime_state_id.to_string(),
                        conversation_epoch: Some(epoch.authority.epoch),
                        expected_scope_revision: scope_revision,
                        expected_state_version: expected.clone(),
                        checkpoint: checkpoint.clone(),
                    },
                ),
            )
            .await
            .map_err(|_| persistence_deadline_error("pending checkpoint prepare"))??;
            match prepare_receipt.status {
                crate::state::RuntimeCheckpointCasStatus::Applied
                | crate::state::RuntimeCheckpointCasStatus::AlreadyCurrent => {
                    *self.snapshot.runtime_state_version.lock().await =
                        Some(prepare_receipt.version.clone());
                    let prepared_state = crate::state::ManagedRuntimeStateSnapshot {
                        scope: prepare_receipt.scope,
                        runtime_state_id: runtime_state_id.to_string(),
                        version: prepare_receipt.version,
                        checkpoint: Some(checkpoint),
                    };
                    let outcome = crate::state::settle_loaded_pending_transcript_projection(
                        conversation_store.as_ref(),
                        runtime_store.as_ref(),
                        scope_id,
                        runtime_state_id,
                        prepared_state,
                        crate::state::TranscriptProjectionDispatch::CurrentAttempt,
                        self.call_context,
                    )
                    .await?;
                    if outcome.settlement.status
                        == crate::memory::TranscriptProjectionSettlementStatus::Settled
                    {
                        if let Some(cursor) = outcome.settled_cursor {
                            self.snapshot
                                .transcript_projection_cursor
                                .lock()
                                .await
                                .restore(cursor);
                        }
                        *self.snapshot.runtime_state_version.lock().await =
                            outcome.committed_version;
                        self.snapshot
                            .record_checkpoint_event(runtime_state_id)
                            .await;
                    }
                    Ok(outcome.settlement)
                }
                crate::state::RuntimeCheckpointCasStatus::RevisionConflict => {
                    let (_, _, new_current) = self
                        .current_runtime_state(runtime_store.as_ref(), scope_id, runtime_state_id)
                        .await?;
                    if let Some(state) = new_current.as_ref()
                        && let Some(checkpoint) = state.checkpoint.as_ref()
                    {
                        let payload = checkpoint.restore_managed_runtime_payload()?;
                        if payload.pending_transcript_projection.is_some() {
                            let prior = crate::state::settle_loaded_pending_transcript_projection(
                                conversation_store.as_ref(),
                                runtime_store.as_ref(),
                                scope_id,
                                runtime_state_id,
                                state.clone(),
                                crate::state::TranscriptProjectionDispatch::RetryDurablePending,
                                self.call_context,
                            )
                            .await?
                            .settlement;
                            if prior.status
                                != crate::memory::TranscriptProjectionSettlementStatus::Settled
                            {
                                return Ok(prior);
                            }
                        }
                    }
                    *self.snapshot.runtime_state_version.lock().await = None;
                    Ok(settlement(
                        crate::memory::TranscriptProjectionSettlementStatus::Blocked,
                        Some(batch.operation_id),
                        Some(scope_id.to_string()),
                        Some(generation_id.to_string()),
                        prepare_attempt,
                        Some(crate::memory::TranscriptProjectionErrorClass::RevisionConflict),
                        Some(
                            "runtime revision changed; rehydrate context before retry".to_string(),
                        ),
                    ))
                }
                crate::state::RuntimeCheckpointCasStatus::GenerationRetired
                | crate::state::RuntimeCheckpointCasStatus::ScopeFenced => Ok(settlement(
                    crate::memory::TranscriptProjectionSettlementStatus::Conflict,
                    Some(batch.operation_id),
                    Some(scope_id.to_string()),
                    Some(generation_id.to_string()),
                    prepare_attempt,
                    Some(crate::memory::TranscriptProjectionErrorClass::SemanticConflict),
                    Some(format!(
                        "pending checkpoint prepare was fenced: {:?}",
                        prepare_receipt.status
                    )),
                )),
            }
        }
    }

    async fn reconcile_pending_projection(
        &self,
    ) -> crate::error::Result<Option<crate::memory::TranscriptProjectionSettlement>> {
        let Some((conversation_store, runtime_store)) = self.managed_stores() else {
            return Ok(None);
        };
        let (scope_id, runtime_state_id, _generation_id) = self.identities()?;
        let conversation_authority = tokio::time::timeout_at(
            self.deadline,
            conversation_store.get_projection_authority_with_context(self.call_context, scope_id),
        )
        .await
        .map_err(|_| persistence_deadline_error("conversation authority admission load"))??;
        let scope_authority = tokio::time::timeout_at(
            self.deadline,
            runtime_store.load_scope_authority_with_context(self.call_context, scope_id),
        )
        .await
        .map_err(|_| persistence_deadline_error("runtime scope admission load"))??;
        if conversation_authority.as_ref().is_some_and(|value| {
            value.lifecycle == crate::memory::ConversationProjectionLifecycle::Deleted
        }) {
            return Err(
                echo_core::error::RuntimeStateError::TranscriptProjectionBlocked {
                    status: "Conflict".to_string(),
                    reason: format!(
                        "conversation scope {scope_id} is deleted and must be explicitly recreated"
                    ),
                }
                .into(),
            );
        }
        if let Some(scope) = scope_authority.as_ref() {
            let conversation_is_newer_live = conversation_authority.as_ref().is_some_and(|value| {
                value.lifecycle == crate::memory::ConversationProjectionLifecycle::Live
                    && scope
                        .conversation_epoch
                        .is_none_or(|epoch| value.epoch > epoch)
            });
            let invalid_lifecycle = match scope.lifecycle {
                crate::state::RuntimeScopeLifecycle::Active => match scope.conversation_epoch {
                    Some(epoch) => !conversation_authority.as_ref().is_some_and(|value| {
                        value.lifecycle == crate::memory::ConversationProjectionLifecycle::Live
                            && value.epoch == epoch
                    }),
                    None => scope.revision > 0,
                },
                crate::state::RuntimeScopeLifecycle::Retiring => true,
                crate::state::RuntimeScopeLifecycle::Tombstoned => !conversation_is_newer_live,
            };
            if invalid_lifecycle {
                return Err(
                    echo_core::error::RuntimeStateError::TranscriptProjectionBlocked {
                        status: "Conflict".to_string(),
                        reason: format!(
                            "runtime scope {scope_id} is not admissible in lifecycle {:?}",
                            scope.lifecycle
                        ),
                    }
                    .into(),
                );
            }
        }
        let (_, _, current) = self
            .current_runtime_state(runtime_store.as_ref(), scope_id, runtime_state_id)
            .await?;
        let Some(state) = current else {
            return Ok(None);
        };
        let Some(checkpoint) = state.checkpoint.as_ref() else {
            return Ok(None);
        };
        let payload = checkpoint.restore_managed_runtime_payload()?;
        if payload.pending_transcript_projection.is_none() {
            return Ok(None);
        }
        *self.snapshot.runtime_state_version.lock().await = None;
        let outcome = crate::state::settle_loaded_pending_transcript_projection(
            conversation_store.as_ref(),
            runtime_store.as_ref(),
            scope_id,
            runtime_state_id,
            state,
            crate::state::TranscriptProjectionDispatch::RetryDurablePending,
            self.call_context,
        )
        .await?;
        Ok(Some(outcome.settlement))
    }

    async fn save_legacy_checkpoint(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        blocked_reason: Option<String>,
    ) -> crate::error::Result<()> {
        let Some(store) = self.snapshot.state_store.as_ref() else {
            return Ok(());
        };
        let Some(runtime_state_id) = self.snapshot.config.runtime_state_id.as_ref() else {
            return Ok(());
        };
        let checkpoint = self
            .build_checkpoint(context, blocked_reason, None, None)
            .await?;
        let scope_id = self
            .snapshot
            .config
            .conversation_id
            .as_deref()
            .unwrap_or(runtime_state_id);
        let versioned = self
            .snapshot
            .runtime_state_version
            .lock()
            .await
            .as_ref()
            .is_some_and(|version| {
                matches!(version, crate::state::RuntimeStateVersion::Managed { .. })
            });
        if versioned {
            // 当前revisioned owner的保存仍须沿原CAS，不能退回无版本save覆盖。
            let (scope_revision, expected, current) = self
                .current_runtime_state_observed(store.as_ref(), scope_id, runtime_state_id, true)
                .await?;
            // 只能保存本context实际hydrate的版本，不能借fresh expected覆盖并发后继。
            self.require_hydrated_base(current.as_ref()).await?;
            let conversation_epoch = current
                .as_ref()
                .and_then(|state| state.scope.conversation_epoch);
            let mut checkpoint = checkpoint;
            if let Some(current) = current.as_ref() {
                // runtime-only恢复亦保留原cursor lineage；新view沿既有context-only派生，
                // 不把未投递到ConversationStore的内容伪造为耐久transcript事实。
                let messages = checkpoint.restore_messages()?;
                checkpoint.messages_json =
                    derive_context_view_checkpoint(current, messages)?.messages_json;
            }
            let receipt = self
                .save_frozen_checkpoint_observed(
                    store.as_ref(),
                    crate::state::RuntimeCheckpointCasRequest {
                        scope_id: scope_id.to_string(),
                        runtime_state_id: runtime_state_id.clone(),
                        conversation_epoch,
                        expected_scope_revision: scope_revision,
                        expected_state_version: expected,
                        checkpoint,
                        managed_import: None,
                    },
                    true,
                )
                .await?;
            *self.snapshot.runtime_state_version.lock().await = Some(receipt.version);
        } else {
            store
                .save_checkpoint_for_scope(scope_id, &checkpoint)
                .await?;
        }
        self.snapshot
            .record_checkpoint_event(runtime_state_id)
            .await;
        Ok(())
    }
}

fn persistence_configuration_error(message: impl Into<String>) -> crate::error::ReactError {
    crate::error::ConfigError::ConfigFileError(message.into()).into()
}

fn persistence_deadline_error(operation: &str) -> crate::error::ReactError {
    echo_core::error::RuntimeStateError::DeadlineExceeded(operation.to_string()).into()
}

fn settlement(
    status: crate::memory::TranscriptProjectionSettlementStatus,
    operation_id: Option<String>,
    conversation_id: Option<String>,
    generation_id: Option<String>,
    attempt: u32,
    error_class: Option<crate::memory::TranscriptProjectionErrorClass>,
    detail: Option<String>,
) -> crate::memory::TranscriptProjectionSettlement {
    crate::memory::TranscriptProjectionSettlement {
        status,
        operation_id,
        conversation_id,
        generation_id,
        attempt,
        error_class,
        detail,
    }
}

impl AgentRunSnapshot {
    pub(crate) fn mark_transcript_settlement_observed(&self) {
        self.transcript_settlement_observed
            .store(true, std::sync::atomic::Ordering::Release);
    }

    fn reset_transcript_settlement_observed(&self) {
        self.transcript_settlement_observed
            .store(false, std::sync::atomic::Ordering::Release);
    }

    pub(crate) fn transcript_settlement_was_observed(&self) -> bool {
        self.transcript_settlement_observed
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Create a snapshot from a [`super::ReactAgent`].
    pub fn from_agent(agent: &super::ReactAgent) -> Self {
        Self::from_agent_with_invocation(
            agent,
            &echo_core::agent::AgentInvocationContext::default(),
        )
    }

    /// 所有调用级状态从一个显式值复制；plain快照使用默认invocation。
    /// 不读取Agent共享可变run状态，也不以trace/turn id补齐缺失业务run id。
    pub fn from_agent_with_invocation(
        agent: &super::ReactAgent,
        invocation: &echo_core::agent::AgentInvocationContext,
    ) -> Self {
        let now = std::time::SystemTime::now();
        let retained_model_profile = agent
            .config
            .model_profile
            .as_ref()
            .map(|resolution| resolution.refresh_at(now));
        let model_profile = agent
            .llm_client()
            .map(|client| {
                crate::agent::react::merge_model_profile_resolution(
                    retained_model_profile.as_ref(),
                    client.model_profile_resolution(),
                )
            })
            .or(retained_model_profile);
        let mut config = RuntimeConfig::from_agent_config_with_model_profile(
            &agent.config,
            model_profile.as_ref(),
        );
        let configured_runtime_state_id = config.runtime_state_id.clone();
        config.input_modalities = agent
            .llm_config()
            .map(|llm_config| llm_config.input_modalities.clone());
        if let Some(working_dir) = invocation.working_dir.as_ref() {
            config.working_dir = Some(working_dir.clone());
        }
        if let Some(run_budget) = invocation.run_budget.as_ref() {
            config.run_budget = run_budget.clone();
        }
        let runtime = invocation.runtime.as_ref();
        let external_delegation_policy = runtime.and_then(|context| context.delegation_policy);
        if let Some(conversation_id) = runtime.and_then(|context| context.conversation_id.clone()) {
            config.conversation_id = Some(conversation_id);
        }
        config.runtime_state_id =
            effective_runtime_state_id(configured_runtime_state_id.as_deref(), Some(invocation))
                .map(str::to_string);
        let transcript_generation_id = invocation
            .transcript_generation_id
            .clone()
            .or_else(|| config.runtime_state_id.clone());
        let tools = ToolRuntime::from_agent_with_tool_scope(
            agent,
            model_profile.as_ref(),
            invocation.disabled_tools.as_ref(),
            invocation.visible_tools.as_ref(),
            invocation.allowed_tools.as_ref(),
            invocation.tool_access,
            external_delegation_policy,
        );
        let skill_activation = agent.tools.skill_registry.activation_handle();
        config.plan_mode = tools.is_plan_mode();
        Self {
            config: Arc::new(config),
            context: agent.memory.context.clone(),
            tools: Arc::new(tools),
            guard: Arc::new(GuardRuntime::from_agent(agent)),
            snapshot_manager: agent.memory.snapshot_manager.clone(),
            transcript_generation_id,
            transcript_projection_cursor: Arc::clone(&agent.memory.transcript_projection_cursor),
            runtime_state_version: Arc::clone(&agent.memory.runtime_state_version),
            runtime_state_hydration: Arc::clone(&agent.memory.runtime_state_hydration),
            transcript_settlement_observed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            client: agent.client().clone(),
            llm_client: agent.llm_client().cloned(),
            thinking: agent.thinking().cloned(),
            cancel_token: invocation.cancel.clone().or_else(|| {
                runtime
                    .and_then(|value| value.cancel.as_ref())
                    .map(|cancel| cancel.as_ref().clone())
            }),
            turn_steer_mailbox: Arc::clone(&agent.turn_steer_mailbox),
            recently_read_files: Arc::clone(&agent.recently_read_files),
            run_store: agent.run_store.clone(),
            diagnostic_delivery_observer: agent.diagnostic_delivery_observer.clone(),
            current_run_id: runtime.and_then(|context| context.run_id.clone()),
            trace_run_id: None,
            pending_checkpoint_resume: None,
            executing_tool_call_ids: Arc::new(std::sync::Mutex::new(
                std::collections::HashSet::new(),
            )),
            current_turn_id: runtime.and_then(|context| context.turn_id.clone()),
            turn_steer_incarnation: None,
            current_message_id: runtime.and_then(|context| context.message_id.clone()),
            current_message: None,
            current_execution_id: runtime.and_then(|context| context.execution_id.clone()),
            external_cancel: runtime
                .and_then(|value| value.cancel.clone())
                .or_else(|| invocation.cancel.clone().map(Arc::new)),
            external_trace_sink: runtime.and_then(|context| context.trace_sink.clone()),
            external_delegation_policy,
            subagent_lineage: runtime.and_then(|context| context.subagent_lineage.clone()),
            external_uplink: runtime.and_then(|context| context.uplink.clone()),
            resource_guards: {
                let mut guards = runtime
                    .map(|runtime| runtime.resource_guards.clone())
                    .unwrap_or_default();
                guards.extend(invocation.resource_guards.iter().cloned());
                guards
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
            memory_store: agent.memory.store.clone(),
            critic: agent.critic.clone(),
            tool_execution_pipeline: agent.tool_execution_pipeline.clone(),
            memory_layer_manager: agent.memory_layer_manager.clone(),
            pre_model_context_projector: agent
                .pre_model_context_projector
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
            skill_mutation_authority: agent.skill_mutation_authority.clone(),
            skill_activation,
        }
    }

    fn active_skill_names(&self) -> Vec<String> {
        self.skill_activation.activated_names()
    }

    /// Persist one best-effort observation for every skill active in this
    /// invocation. Telemetry is observability, not execution authority: a
    /// missing store or failed write must never change the tool outcome.
    fn record_skill_telemetry(
        &self,
        tool_name: &str,
        duration_ms: u64,
        success: bool,
        error: Option<&str>,
    ) {
        let Some(store) = self.memory_store.clone() else {
            return;
        };
        let skill_names = self.active_skill_names();
        if skill_names.is_empty() {
            return;
        }
        let session_id = self.config.session_id.clone().unwrap_or_default();
        let tool_name = tool_name.to_string();
        let error_message = error.map(str::to_string);
        let skill_authority = self.skill_mutation_authority.clone();
        let activated_at = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
        tokio::spawn(async move {
            let telemetry_store = crate::skill_telemetry::SkillTelemetryStore::new(store);
            for skill_name in skill_names {
                let record = crate::skill_telemetry::SkillExecutionRecord {
                    skill_name: skill_name.clone(),
                    session_id: session_id.clone(),
                    activated_at,
                    duration_ms,
                    tools_used: vec![tool_name.clone()],
                    tool_calls_count: 1,
                    success,
                    error_message: error_message.clone(),
                };
                if let Err(write_error) = telemetry_store.record_execution(&record).await {
                    tracing::warn!(
                        skill = %skill_name,
                        error = %write_error,
                        "skill telemetry write failed"
                    );
                }
                if let Some(authority) = skill_authority.as_ref() {
                    let usage = crate::evolution::SkillUsageHandle::new(authority.clone());
                    if let Err(error) = usage.record_usage(&skill_name).await {
                        tracing::warn!(
                            skill = %skill_name,
                            error = %error,
                            "skill usage lifecycle mutation failed"
                        );
                    }
                }
            }
        });
    }

    // ── Trace helpers ──────────────────────────────────────────────

    fn report_diagnostic_delivery_failure(&self, failure: DiagnosticDeliveryFailure) {
        crate::audit::report_diagnostic_delivery_failure(
            self.diagnostic_delivery_observer.clone(),
            failure,
        );
    }

    /// Persist one audit event through the optional Agent callback backend.
    pub(crate) async fn record_audit_event(&self, mut event: crate::audit::AuditEvent) {
        let Some(logger) = self.guard.audit_logger.as_ref() else {
            return;
        };
        event.apply_retention(&echo_core::utils::retention::ContentRetentionPolicy::default());
        let record_id = event.trace_id.clone().or_else(|| event.session_id.clone());
        if let Err(error) = logger.log(event).await {
            self.report_diagnostic_delivery_failure(DiagnosticDeliveryFailure::new(
                DiagnosticRecordKind::Audit,
                DiagnosticDeliveryOperation::Record,
                record_id,
                error.to_string(),
            ));
        }
    }

    /// Record a trace event if a run store is attached.
    pub async fn record_event(&self, mut event: RunEvent) {
        event.apply_retention(&echo_core::utils::retention::ContentRetentionPolicy::default());
        if let Some(ref store) = self.run_store
            && let Some(ref run_id) = self.trace_run_id
            && let Err(error) = store.append_event(run_id, event).await
        {
            self.report_diagnostic_delivery_failure(DiagnosticDeliveryFailure::new(
                DiagnosticRecordKind::Trace,
                DiagnosticDeliveryOperation::Append,
                Some(run_id.clone()),
                error.to_string(),
            ));
        }
    }

    /// Project a fact from the tool that owns the effect into this invocation's trace.
    pub(crate) async fn record_tool_effect(
        &self,
        effect: &echo_core::tools::ToolEffect,
        tool_name: &str,
        call_id: &str,
    ) {
        let event = match effect {
            echo_core::tools::ToolEffect::FileRead { path } => RunEvent::FileRead {
                tool: tool_name.to_string(),
                path: path.clone(),
            },
            echo_core::tools::ToolEffect::FileEdit { path } => RunEvent::FileEdit {
                tool: tool_name.to_string(),
                path: path.clone(),
            },
            echo_core::tools::ToolEffect::TestRun {
                command,
                passed,
                failure_count,
            } => RunEvent::TestRun {
                command: command.clone(),
                passed: *passed,
                failure_count: *failure_count,
            },
            echo_core::tools::ToolEffect::SubagentRun {
                agent_name,
                task,
                outcome,
            } => RunEvent::SubagentRun {
                call_id: Some(call_id.to_string()),
                agent_name: agent_name.clone(),
                task: task.clone(),
                outcome: outcome.clone(),
            },
        };
        self.record_event(event).await;
    }

    pub(crate) fn mark_tool_execution_started(&self, call_id: &str) {
        self.executing_tool_call_ids
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(call_id.to_string());
    }

    fn tool_execution_started(&self, call_id: &str) -> bool {
        self.executing_tool_call_ids
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(call_id)
    }

    pub(crate) async fn settle_interrupted_tool_call(
        &self,
        call_id: &str,
        tool_name: &str,
        input: &serde_json::Value,
        category: crate::tools::ToolFailureCategory,
        message: &str,
        known_not_started: bool,
    ) -> crate::tools::ToolResult {
        // The set retains history for physical owners through this turn and
        // can contain an earlier, completed attempt with the same model ID.
        // Only the caller's original current-wave admission can prove that
        // this specific attempt was never started; do not clear that history.
        let started = !known_not_started && self.tool_execution_started(call_id);
        if !started {
            // A pre-execution stage or future wave never reached ExecuteStage.
            // The invocation-local set is authoritative; trace persistence is
            // only a projection and may be unavailable or eventually visible.
            self.record_event(RunEvent::new_tool_call(
                call_id.to_string(),
                tool_name.to_string(),
                Some(input.clone()),
                None,
                0,
            ))
            .await;
            self.record_event(RunEvent::ToolExecutionSkipped {
                call_id: call_id.to_string(),
                name: tool_name.to_string(),
                reason: category.as_str().to_string(),
            })
            .await;
        }
        // ExecuteStage marks the invocation-local set before its first await
        // and before invoking ToolManager. Callers settle only unadmitted
        // waves or executions whose original future has already ended; no
        // live future can enter ExecuteStage after this fact is sampled.
        // Trace/event visibility is not evidence that an effect did not run.
        let failure = if started {
            crate::tools::ToolFailure::new(category)
                .with_side_effect(crate::tools::ToolSideEffect::Possible)
                .with_postcondition(
                    "verify the external effect before retrying this interrupted tool call",
                )
        } else {
            crate::tools::ToolFailure::new(category)
                .with_side_effect(crate::tools::ToolSideEffect::None)
                .with_postcondition("not executed: reconsider this tool call before starting it")
        };
        let result = crate::tools::ToolResult::error(message).with_failure(failure.clone());
        self.record_event(crate::trace::RunEvent::ToolResult {
            call_id: call_id.to_string(),
            name: tool_name.to_string(),
            success: false,
            output_preview: Some(String::new()),
            output_truncated: false,
            duration_ms: 0,
            original_bytes: 0,
            returned_bytes: 0,
            estimated_tokens: 0,
            output_handling: None,
            artifact: None,
        })
        .await;
        self.record_event(crate::trace::RunEvent::ToolError {
            call_id: call_id.to_string(),
            name: tool_name.to_string(),
            message: message.to_string(),
            failure: Some(failure),
        })
        .await;
        self.record_audit_event(crate::audit::AuditEvent::now(
            self.config.session_id.clone(),
            self.config.agent_name.clone(),
            crate::audit::AuditEventType::ToolCall {
                call_id: Some(call_id.to_string()),
                tool: tool_name.to_string(),
                input: input.clone(),
                output: message.to_string(),
                success: false,
                duration_ms: 0,
            },
        ))
        .await;
        let error = crate::error::ReactError::Other(message.to_string());
        for callback in &self.config.callbacks {
            callback
                .on_tool_interrupted_with_id(
                    &self.config.agent_name,
                    call_id,
                    tool_name,
                    input,
                    &error,
                )
                .await;
        }
        result
    }

    /// Finalize the current trace run (completed or failed).
    pub async fn finalize_run(&self, status: RunStatus, output: Option<&str>, error: Option<&str>) {
        if let Some(ref store) = self.run_store
            && let Some(ref run_id) = self.trace_run_id
        {
            match store.finalize_run(run_id, status, output, error).await {
                Ok(true) => {}
                Ok(false) => {
                    self.report_diagnostic_delivery_failure(DiagnosticDeliveryFailure::new(
                        DiagnosticRecordKind::Trace,
                        DiagnosticDeliveryOperation::Load,
                        Some(run_id.clone()),
                        "trace run not found during finalization",
                    ))
                }
                Err(finalize_error) => {
                    self.report_diagnostic_delivery_failure(DiagnosticDeliveryFailure::new(
                        DiagnosticRecordKind::Trace,
                        DiagnosticDeliveryOperation::Finalize,
                        Some(run_id.clone()),
                        finalize_error.to_string(),
                    ))
                }
            }
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
    /// Persists the [`crate::state::AgentCheckpoint`] runtime state (messages,
    /// active skills, and blocked reason) so an in-flight conversation can resume
    /// across process restarts.
    ///
    /// Silently no-ops if no state store or runtime-state identity is configured.
    pub async fn save_runtime_checkpoint(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        blocked_reason: Option<String>,
    ) -> crate::error::Result<()> {
        validate_transcript_generation_identity(
            self.config.runtime_state_id.as_deref(),
            self.transcript_generation_id.as_deref(),
        )?;
        if self.conversation_store.is_none() {
            return AgentPersistenceCoordinator::new(self)?
                .save_checkpoint(context, blocked_reason, None)
                .await;
        }
        let settlement = self
            .save_transcript_projection(context, blocked_reason)
            .await?;
        match settlement.status {
            crate::memory::TranscriptProjectionSettlementStatus::Settled => Ok(()),
            crate::memory::TranscriptProjectionSettlementStatus::Deferred => Err(
                echo_core::error::RuntimeStateError::TranscriptProjectionDeferred {
                    operation_id: settlement.operation_id,
                    reason: settlement.detail.unwrap_or_else(|| {
                        "checkpoint retained a durable transcript projection debt".to_string()
                    }),
                }
                .into(),
            ),
            crate::memory::TranscriptProjectionSettlementStatus::Blocked
            | crate::memory::TranscriptProjectionSettlementStatus::Conflict => {
                Err(transcript_settlement_admission_error(&settlement))
            }
        }
    }

    /// Settle a durable pending transcript effect before hydration or admission.
    pub(crate) async fn reconcile_pending_transcript_projection(
        &self,
    ) -> crate::error::Result<Option<crate::memory::TranscriptProjectionSettlement>> {
        let settlement = AgentPersistenceCoordinator::new(self)?
            .reconcile_pending_projection()
            .await?;
        if let Some(settlement) = settlement.as_ref() {
            self.record_event(crate::trace::RunEvent::TranscriptProjectionSettlement {
                settlement: settlement.clone(),
            })
            .await;
        }
        Ok(settlement)
    }

    pub(crate) async fn observe_persistence_failure(
        &self,
        error: &crate::error::ReactError,
    ) -> crate::memory::TranscriptProjectionSettlement {
        let (classified_status, error_class) =
            crate::state::classify_transcript_persistence_error(error);
        let status = if classified_status
            == crate::memory::TranscriptProjectionSettlementStatus::Deferred
            && matches!(
                error,
                crate::error::ReactError::RuntimeState(inner)
                    if matches!(
                        inner.as_ref(),
                        echo_core::error::RuntimeStateError::TranscriptProjectionDeferred { .. }
                    )
            ) {
            crate::memory::TranscriptProjectionSettlementStatus::Deferred
        } else {
            crate::memory::TranscriptProjectionSettlementStatus::Blocked
        };
        let operation_id = match error {
            crate::error::ReactError::RuntimeState(inner) => match inner.as_ref() {
                echo_core::error::RuntimeStateError::TranscriptProjectionDeferred {
                    operation_id,
                    ..
                } => operation_id.clone(),
                _ => None,
            },
            _ => None,
        };
        let settlement = settlement(
            status,
            operation_id,
            self.config.conversation_id.clone(),
            self.transcript_generation_id.clone(),
            0,
            Some(error_class),
            Some(error.to_string()),
        );
        self.record_event(crate::trace::RunEvent::TranscriptProjectionSettlement {
            settlement: settlement.clone(),
        })
        .await;
        settlement
    }

    async fn record_checkpoint_event(&self, runtime_state_id: &str) {
        self.record_event(crate::trace::RunEvent::Checkpoint {
            id: format!(
                "checkpoint:{}:{}",
                runtime_state_id,
                chrono::Utc::now().timestamp_millis()
            ),
        })
        .await;
        tracing::debug!(
            runtime_state_id,
            "Runtime checkpoint compare-and-save settled"
        );
    }

    /// Save the user-visible transcript projection to the [`ConversationStore`](crate::memory::ConversationStore).
    ///
    /// Projects user-visible messages into an epoch-fenced atomic batch while
    /// saving the full runtime `Message` list in the same revisioned
    /// checkpoint. Managed `save_runtime_checkpoint` calls this coordinator so
    /// it cannot advance runtime state without a settled projection or durable
    /// pending marker.
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
        blocked_reason: Option<String>,
    ) -> crate::error::Result<crate::memory::TranscriptProjectionSettlement> {
        self.reset_transcript_settlement_observed();
        let settlement = AgentPersistenceCoordinator::new(self)?
            .settle_transcript_projection(context, blocked_reason)
            .await?;
        if self.conversation_store.is_some() {
            self.record_event(crate::trace::RunEvent::TranscriptProjectionSettlement {
                settlement: settlement.clone(),
            })
            .await;
        }
        Ok(settlement)
    }

    /// Manual compression shares the same durable pre-compaction safe point.
    pub(crate) async fn prepare_manual_context_compaction(
        &self,
    ) -> crate::error::Result<PreparedContextCompaction<'_>> {
        if self.conversation_store.is_some() {
            let settlement = self.save_transcript_projection(&self.context, None).await?;
            if settlement.status != crate::memory::TranscriptProjectionSettlementStatus::Settled {
                return Err(transcript_settlement_admission_error(&settlement));
            }
        }
        self.prepare_context_compaction().await
    }

    /// Prepare only after the caller has settled the original visible facts.
    pub(crate) async fn prepare_context_compaction(
        &self,
    ) -> crate::error::Result<PreparedContextCompaction<'_>> {
        // Store callbacks never run while these simultaneous guards hold
        // Context. Shared state follows Context -> cursor -> version -> hydration.
        let mut context = self.context.lock().await;
        let cursor = self.transcript_projection_cursor.lock().await;
        let version = self.runtime_state_version.lock().await;
        let hydration = self.runtime_state_hydration.lock().await;
        if self.conversation_store.is_some() {
            let generation = self
                .transcript_generation_id
                .as_deref()
                .ok_or_else(|| persistence_configuration_error("missing transcript generation"))?;
            let scope =
                self.config.conversation_id.as_deref().ok_or_else(|| {
                    persistence_configuration_error("missing conversation identity")
                })?;
            let visible = crate::memory::project_messages(
                scope,
                &filter_user_visible_transcript(context.messages()),
            )?;
            let current = transcript_projection_messages(&visible)?;
            let view = cursor.checkpoint_for(generation).active_view()?;
            if current.len() != view.len()
                || current
                    .iter()
                    .zip(view)
                    .any(|(message, (digest, _))| message.digest != digest)
            {
                return Err(
                    echo_core::error::RuntimeStateError::ManagedStateRequiresCas(
                        "context changed after transcript settlement".to_string(),
                    )
                    .into(),
                );
            }
        }
        let original_messages = context.messages().to_vec();
        let original_context = context.prepare_context_transform()?;
        let original_cursor = cursor.clone();
        let original_hydration = hydration.clone();
        Ok(PreparedContextCompaction {
            snapshot: self,
            context: Some(context),
            cursor,
            version,
            hydration,
            original_context,
            original_messages,
            original_cursor,
            original_hydration,
            restore_context: true,
            completed: false,
        })
    }

    // ── Tool execution helpers (delegated from Pipeline stages) ─────

    /// Check tool approval via PermissionService and retain the exact approval
    /// receipt for the final effective invocation.
    #[cfg(feature = "human-loop")]
    pub async fn check_tool_approval_with_receipt(
        &self,
        request_id: &str,
        tool_name: &str,
        input: &serde_json::Value,
        permission_mode_override: Option<echo_core::tools::permission::PermissionMode>,
    ) -> std::result::Result<
        Option<echo_orchestration::human_loop::PermissionCheck>,
        echo_core::error::ReactError,
    > {
        let permissions = self
            .tools
            .tool_manager
            .get_tool(tool_name)
            .map(|tool| tool.permissions())
            .unwrap_or_default();
        self.check_tool_approval_with_intent(
            request_id,
            tool_name,
            input,
            permission_mode_override,
            None,
            &permissions,
        )
        .await
    }

    /// `Some` retains an explicit interaction requirement, including an empty
    /// suggestion list for RequireApproval. Identity and cancellation retain
    /// the same owner as ordinary permission checks.
    #[cfg(feature = "human-loop")]
    pub(crate) async fn check_tool_approval_with_intent(
        &self,
        request_id: &str,
        tool_name: &str,
        input: &serde_json::Value,
        permission_mode_override: Option<echo_core::tools::permission::PermissionMode>,
        approval_suggestions: Option<&[String]>,
        permissions: &[echo_core::tools::permission::ToolPermission],
    ) -> std::result::Result<
        Option<echo_orchestration::human_loop::PermissionCheck>,
        echo_core::error::ReactError,
    > {
        if let Some(ref service) = self.permission_service {
            let pending = {
                let mut rules = self.pending_permission_rules.lock().await;
                std::mem::take(&mut *rules)
            };
            if !pending.is_empty() {
                service.add_rules(pending).await;
            }
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
            let check = async {
                if let Some(suggestions) = approval_suggestions {
                    service
                        .request_approval_with_context(
                            tool_name,
                            input,
                            permissions,
                            permission_mode_override,
                            Some(&permission_context),
                            suggestions,
                        )
                        .await
                } else {
                    service
                        .check_with_permissions_result_in_mode_and_context(
                            tool_name,
                            input,
                            permissions,
                            permission_mode_override,
                            Some(&permission_context),
                        )
                        .await
                }
            };
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
            self.record_permission_decision(tool_name, &decision.decision, "permission_service")
                .await;
            match decision.decision {
                echo_core::tools::permission::PermissionDecision::Allow => Ok(Some(decision)),
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
        } else if approval_suggestions.is_some() {
            Err(echo_core::error::ReactError::Other(format!(
                "Tool '{tool_name}' explicitly requires approval but no PermissionService is configured"
            )))
        } else {
            Ok(None)
        }
    }

    /// Check tool approval via PermissionService (no-op when human-loop feature is disabled).
    #[cfg(not(feature = "human-loop"))]
    pub async fn check_tool_approval_with_receipt(
        &self,
        _request_id: &str,
        _tool_name: &str,
        _input: &serde_json::Value,
        _permission_mode_override: Option<echo_core::tools::permission::PermissionMode>,
    ) -> std::result::Result<
        Option<echo_orchestration::human_loop::PermissionCheck>,
        echo_core::error::ReactError,
    > {
        Ok(None)
    }

    #[cfg(not(feature = "human-loop"))]
    pub(crate) async fn check_tool_approval_with_intent(
        &self,
        _request_id: &str,
        tool_name: &str,
        _input: &serde_json::Value,
        _permission_mode_override: Option<echo_core::tools::permission::PermissionMode>,
        approval_suggestions: Option<&[String]>,
        _permissions: &[echo_core::tools::permission::ToolPermission],
    ) -> std::result::Result<
        Option<echo_orchestration::human_loop::PermissionCheck>,
        echo_core::error::ReactError,
    > {
        if approval_suggestions.is_some() {
            return Err(echo_core::error::ReactError::Other(format!(
                "Tool '{tool_name}' explicitly requires approval but the human-loop feature is disabled"
            )));
        }
        Ok(None)
    }

    /// Backward-compatible permission check that exposes only any rewritten
    /// input.  New execution paths should use
    /// [`Self::check_tool_approval_with_receipt`] so effectful tools retain the
    /// exact invocation approval proof.
    pub async fn check_tool_approval(
        &self,
        request_id: &str,
        tool_name: &str,
        input: &serde_json::Value,
        permission_mode_override: Option<echo_core::tools::permission::PermissionMode>,
    ) -> std::result::Result<Option<serde_json::Value>, echo_core::error::ReactError> {
        self.check_tool_approval_with_receipt(
            request_id,
            tool_name,
            input,
            permission_mode_override,
        )
        .await
        .map(|check| check.and_then(|value| value.updated_input))
    }

    /// 记录调用方已确认的原source及resolved路径；不再用host/snapshot猜来源。
    /// 公开Rust签名显式携带source，自动pipeline也只以真实owned工具证明调用它。
    pub fn record_file_read(
        &self,
        path: &str,
        source: Arc<dyn echo_core::tools::filesystem::FileSystem>,
    ) {
        self.record_file_read_observation(path, Some(source));
    }

    pub(crate) fn record_file_read_observation(
        &self,
        path: &str,
        source: Option<Arc<dyn echo_core::tools::filesystem::FileSystem>>,
    ) {
        let mut files = self
            .recently_read_files
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let observations = files.entry(path.to_string()).or_default();
        let existing =
            observations
                .iter_mut()
                .find(|(old, _)| match (old.as_ref(), source.as_ref()) {
                    (Some(old), Some(new)) => same_file_tracking_source(old, new),
                    (None, None) => true,
                    _ => false,
                });
        if let Some((_, at)) = existing {
            *at = std::time::Instant::now();
        } else {
            observations.push((source, std::time::Instant::now()));
        }
    }

    pub(crate) async fn resolve_file_tracking_path(
        &self,
        tool: Arc<dyn echo_core::tools::Tool>,
        path: &str,
        parameters: &crate::tools::ToolParameters,
        context: &echo_core::tools::ToolContext,
    ) -> crate::error::Result<(
        Arc<dyn echo_core::tools::filesystem::FileSystem>,
        std::path::PathBuf,
    )> {
        let path = path.to_string();
        let parameters = parameters.clone();
        let context = context.clone();
        let guards = self.resource_guards.clone();
        tokio::task::spawn_blocking(move || {
            let _guards = guards;
            tool.resolve_file_tracking_path(&path, &parameters, &context)
        })
        .await
        .map_err(|error| {
            crate::error::ReactError::Other(format!("File tracking task failed: {error}"))
        })?
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
            .check_all(&effective_output, GuardDirection::ToolOutput)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(agent = %self.config.agent_name, error = %e, "Guard check failed, blocking output (fail-closed)");
                if self.guard.audit_logger.is_some() {
                    let event = crate::audit::AuditEvent::now(
                        self.config.session_id.clone(),
                        self.config.agent_name.clone(),
                        crate::audit::AuditEventType::GuardBlock {
                            guard: "guard_manager".to_string(),
                            direction: GuardDirection::ToolOutput,
                            reason: format!("guard check error: {e}"),
                        },
                    );
                    self.record_audit_event(event).await;
                }
                return Some(format!("Output content blocked: guard check error ({e})"));
            }
        };
        match result {
            crate::guard::GuardResult::Block { reason } => {
                tracing::info!(agent = %self.config.agent_name, reason = %reason, "🛡️ Tool output blocked by guard");
                if self.guard.audit_logger.is_some() {
                    let event = crate::audit::AuditEvent::now(
                        self.config.session_id.clone(),
                        self.config.agent_name.clone(),
                        crate::audit::AuditEventType::GuardBlock {
                            guard: "guard_manager".to_string(),
                            direction: GuardDirection::ToolOutput,
                            reason: reason.clone(),
                        },
                    );
                    self.record_audit_event(event).await;
                }
                Some(format!("Output content filtered by safety guard: {reason}"))
            }
            crate::guard::GuardResult::Transform { content, .. } => Some(content),
            crate::guard::GuardResult::Pass | crate::guard::GuardResult::Warn { .. } => {
                (effective_output != output).then_some(effective_output)
            }
        }
    }

    /// Check the final answer at the LLM-to-user boundary.
    ///
    /// Tool results use [`GuardDirection::ToolOutput`]; this separate
    /// direction remains authoritative for the final answer emitted by the
    /// run.  A block or backend error is returned to the terminal owner so
    /// the original answer can never escape the guard boundary.
    pub(crate) async fn check_final_answer_guard(
        &self,
        output: &str,
    ) -> crate::error::Result<String> {
        let Some(guard_manager) = self.guard.guard_manager.as_ref() else {
            return Ok(output.to_string());
        };
        use crate::guard::GuardDirection;
        let result = match guard_manager
            .check_all(output, GuardDirection::Output)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                tracing::error!(
                    agent = %self.config.agent_name,
                    error = %error,
                    "Final answer guard failed closed"
                );
                if self.guard.audit_logger.is_some() {
                    let event = crate::audit::AuditEvent::now(
                        self.config.session_id.clone(),
                        self.config.agent_name.clone(),
                        crate::audit::AuditEventType::GuardBlock {
                            guard: "guard_manager".to_string(),
                            direction: GuardDirection::Output,
                            reason: format!("guard check error: {error}"),
                        },
                    );
                    self.record_audit_event(event).await;
                }
                return Err(error);
            }
        };
        match result {
            crate::guard::GuardResult::Pass | crate::guard::GuardResult::Warn { .. } => {
                Ok(output.to_string())
            }
            crate::guard::GuardResult::Transform { content, .. } => Ok(content),
            crate::guard::GuardResult::Block { reason } => {
                if self.guard.audit_logger.is_some() {
                    let event = crate::audit::AuditEvent::now(
                        self.config.session_id.clone(),
                        self.config.agent_name.clone(),
                        crate::audit::AuditEventType::GuardBlock {
                            guard: "guard_manager".to_string(),
                            direction: GuardDirection::Output,
                            reason: reason.clone(),
                        },
                    );
                    self.record_audit_event(event).await;
                }
                Err(crate::error::ReactError::Other(format!(
                    "Final answer blocked by safety guard: {reason}"
                )))
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
            return ProcessedToolOutput {
                output: model_output,
                truncated: true,
                artifact: Some(artifact),
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
                artifact: None,
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
                artifact: None,
                metadata,
            };
        }

        let notice = format!(
            "\n\n[Output truncated: ~{estimated_tokens} tokens total, {max_tokens} token budget]\n\n"
        );
        let notice = if tokenizer.count_tokens(&notice) <= max_tokens {
            notice
        } else if tokenizer.count_tokens("[Output truncated]") <= max_tokens {
            "[Output truncated]".to_string()
        } else if tokenizer.count_tokens("…") <= max_tokens {
            "…".to_string()
        } else {
            String::new()
        };
        let available_tokens = max_tokens.saturating_sub(tokenizer.count_tokens(&notice));
        let render = |retained_chars: usize| {
            let head_chars = retained_chars.saturating_mul(7) / 10;
            let tail_chars = retained_chars.saturating_sub(head_chars);
            let head: String = output.chars().take(head_chars).collect();
            let tail_reversed: String = output.chars().rev().take(tail_chars).collect();
            let tail: String = tail_reversed.chars().rev().collect();
            format!("{head}{notice}{tail}")
        };
        // The character bound limits candidate size, not token accounting.
        // Count the entire projection, including its notice, so non-ASCII
        // text and tiny budgets cannot exceed the original tokenizer's cap.
        let mut low = 0_usize;
        let mut high = output
            .chars()
            .take(available_tokens.saturating_mul(4))
            .count();
        while low < high {
            let middle = low.saturating_add(high.saturating_sub(low).saturating_add(1) / 2);
            if tokenizer.count_tokens(&render(middle)) <= max_tokens {
                low = middle;
            } else {
                high = middle.saturating_sub(1);
            }
        }
        let truncated_output = render(low);
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
            artifact: None,
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
    /// Uses the unified ToolExecutionPipeline (17 stages) for consistent behavior
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
                audit_error_output: None,
                blocked: false,
                block_reason: None,
                block_failure: None,
                duration_ms: 0,
                plan_mode: self.config.plan_mode,
                permission_decision: None,
                approval_receipt: None,
                authorized_tool: None,
                permission_mode_override: None,
                rewrites: Vec::new(),
                invocation_emitted: false,
                callback_started: false,
                execution_started: false,
                interrupted_execution_error: None,
                stream_tx,
            };

            let pipeline_result = match pipeline.run(&mut ctx, self).await {
                Err(error @ crate::error::ReactError::ExecutionNotSettled { .. }) => {
                    return Err(ToolCallFailure {
                        name: ctx.tool_name,
                        error,
                        result: None,
                    });
                }
                result => result,
            };
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
                        let mut result = ctx.result.unwrap_or_else(|| {
                            ToolResult::failure(failure.category, reason.clone())
                                .with_failure(failure)
                        });
                        if let Some(output) = ctx.output {
                            result.output = output;
                        }
                        self.record_skill_telemetry(
                            tool_name,
                            ctx.duration_ms,
                            false,
                            Some(reason.as_str()),
                        );
                        return Err(ToolCallFailure {
                            name: ctx.tool_name.clone(),
                            error: crate::error::ToolError::ExecutionFailed {
                                tool: ctx.tool_name.clone(),
                                message: reason,
                            }
                            .into(),
                            result: Some(result),
                        });
                    }

                    // Return the complete result after guard and output budgeting.
                    if let Some(mut result) = ctx.result {
                        if let Some(output) = ctx.output {
                            result.output = output;
                        }
                        if result.success {
                            let telemetry_tool_name = ctx.tool_name.clone();
                            self.record_skill_telemetry(
                                &telemetry_tool_name,
                                ctx.duration_ms,
                                true,
                                None,
                            );
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
                        let telemetry_tool_name = ctx.tool_name.clone();
                        self.record_skill_telemetry(
                            &telemetry_tool_name,
                            ctx.duration_ms,
                            false,
                            Some(message.as_str()),
                        );
                        return Err(ToolCallFailure {
                            name: ctx.tool_name.clone(),
                            error: crate::error::ToolError::ExecutionFailed {
                                tool: ctx.tool_name.clone(),
                                message,
                            }
                            .into(),
                            result: Some(result),
                        });
                    }
                    let message = "Pipeline completed without result".to_string();
                    self.record_skill_telemetry(
                        tool_name,
                        ctx.duration_ms,
                        false,
                        Some(message.as_str()),
                    );
                    Err(ToolCallFailure {
                        name: ctx.tool_name,
                        error: crate::error::ReactError::Other(message.clone()),
                        result: Some(ToolResult::failure(
                            crate::tools::ToolFailureCategory::Permanent,
                            message,
                        )),
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
                    let error_message = error.to_string();
                    let result = if !ctx.callback_started
                        && ctx.result.is_none()
                        && !ctx.execution_started
                        && self
                            .cancel_token
                            .as_ref()
                            .is_some_and(|cancel| cancel.is_cancelled())
                    {
                        // The retained pre-execution owner has now really ended.
                        // It never reached execution/trace stages, so use the
                        // original interruption projector exactly once. Typed
                        // unknown was already refused above, before this branch.
                        self.settle_interrupted_tool_call(
                            &ctx.call_id,
                            &ctx.tool_name,
                            &ctx.input,
                            failure.category,
                            &error_message,
                            true,
                        )
                        .await
                    } else {
                        ToolResult::failure(failure.category, error_message.clone())
                            .with_failure(failure)
                    };
                    let telemetry_tool_name = ctx.tool_name.clone();
                    self.record_skill_telemetry(
                        &telemetry_tool_name,
                        ctx.duration_ms,
                        false,
                        Some(error_message.as_str()),
                    );
                    Err(ToolCallFailure {
                        name: ctx.tool_name,
                        result: Some(result),
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
        AgentRunSnapshot, RuntimeConfig, ToolRuntime, TranscriptProjectionCursor,
        filter_user_visible_transcript,
    };
    #[cfg(feature = "mcp")]
    use crate::agent::react::run::context::HookMessageBatches;
    #[cfg(feature = "mcp")]
    use crate::agent::react::run::pipeline::{
        ExecuteStage, PipelineStage, PlanModeStage, ToolExecutionContext,
    };
    use crate::compression::{ContextManager, ContextProjection};
    use crate::error::{ReactError, Result};
    use echo_core::llm::types::Message;
    use echo_core::tools::{Tool, ToolParameters, ToolResult};
    use std::collections::HashSet;
    use std::sync::Arc;

    struct NamedTool(&'static str);

    struct ReadOnlyNamedTool(&'static str);

    #[cfg(feature = "mcp")]
    struct McpPlanProbe {
        name: String,
        executions: Arc<std::sync::atomic::AtomicUsize>,
    }

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

    impl Tool for ReadOnlyNamedTool {
        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> &str {
            "locally classified read-only snapshot policy test tool"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
            echo_core::tools::ToolRiskLevel::ReadOnly
        }

        fn execute<'a>(
            &'a self,
            _parameters: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            Box::pin(async { Ok(ToolResult::success("ok")) })
        }
    }

    #[cfg(feature = "mcp")]
    impl Tool for McpPlanProbe {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "MCP plan-mode capability probe"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn permissions(&self) -> Vec<echo_core::tools::permission::ToolPermission> {
            vec![echo_core::tools::permission::ToolPermission::Write]
        }

        fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
            echo_core::tools::ToolRiskLevel::Standard
        }

        fn execute<'a>(
            &'a self,
            _parameters: ToolParameters,
        ) -> futures::future::BoxFuture<'a, echo_core::error::Result<ToolResult>> {
            self.executions
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(ToolResult::success("mutated")) })
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
                resource_guards: Vec::new(),
                subagent_lineage: None,
                uplink: None,
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

    #[tokio::test]
    async fn transcript_generation_runtime_identity_rejects_checkpoint_write_before_store_mutation()
    -> Result<()> {
        use crate::state::RuntimeStateStore;

        let temp = tempfile::tempdir()?;
        let store = Arc::new(crate::state::FileRuntimeStateStore::new(temp.path())?);
        let config = crate::agent::AgentConfig::new("test-model", "agent", "system")
            .conversation_id("configured-state");
        let mut agent = crate::agent::ReactAgent::new(config);
        agent.set_state_store(store.clone());
        agent
            .memory
            .context
            .lock()
            .await
            .push(Message::user("must not persist".to_string()));
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime_state_id: Some("runtime-a".to_string()),
            transcript_generation_id: Some("runtime-b".to_string()),
            runtime: Some(echo_core::tools::ExternalRunContext {
                conversation_id: Some("product-conversation".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let snapshot = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation);

        let error = match snapshot
            .save_runtime_checkpoint(&agent.memory.context, None)
            .await
        {
            Ok(()) => {
                return Err(ReactError::Other(
                    "mismatched transcript generation unexpectedly saved".to_string(),
                ));
            }
            Err(error) => error,
        };
        assert!(matches!(error, ReactError::RuntimeState(_)));
        assert!(error.to_string().contains("transcript generation identity"));
        assert!(store.get_checkpoint("runtime-a").await?.is_none());
        assert!(
            store
                .runtime_state_ids("product-conversation")
                .await?
                .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn new_projection_safe_point_resets_observation_provenance() -> Result<()> {
        let agent = crate::agent::ReactAgent::new(crate::agent::AgentConfig::new(
            "test-model",
            "agent",
            "system",
        ));
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        snapshot.mark_transcript_settlement_observed();
        assert!(snapshot.transcript_settlement_was_observed());

        snapshot
            .save_transcript_projection(&agent.memory.context, None)
            .await?;

        assert!(!snapshot.transcript_settlement_was_observed());
        Ok(())
    }

    #[tokio::test]
    async fn checkpoint_save_indexes_product_scope_and_exact_reset_reclaims_runtime() -> Result<()>
    {
        use crate::state::RuntimeStateStore;

        let temp = tempfile::tempdir()?;
        let store = Arc::new(crate::state::FileRuntimeStateStore::new(temp.path())?);
        let config = crate::agent::AgentConfig::new("test-model", "agent", "system")
            .conversation_id("runtime-incarnation");
        let mut agent = crate::agent::ReactAgent::new(config);
        agent.set_state_store(store.clone());
        agent
            .memory
            .context
            .lock()
            .await
            .push(Message::user("indexed turn".to_string()));
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime_state_id: Some("runtime-incarnation".to_string()),
            transcript_generation_id: Some("runtime-incarnation".to_string()),
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
                resource_guards: Vec::new(),
                subagent_lineage: None,
                uplink: None,
            }),
            ..Default::default()
        };
        let snapshot = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation);
        snapshot
            .save_runtime_checkpoint(&agent.memory.context, None)
            .await?;

        assert_eq!(
            store.runtime_state_ids("product-conversation").await?,
            vec!["runtime-incarnation".to_string()]
        );
        assert!(store.get_checkpoint("runtime-incarnation").await?.is_some());
        let reset = store
            .clear_runtime_state("product-conversation", "runtime-incarnation")
            .await?;
        assert!(reset.checkpoint_removed);
        assert!(store.get_checkpoint("runtime-incarnation").await?.is_none());
        assert!(
            store
                .runtime_state_ids("product-conversation")
                .await?
                .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn checkpoint_reads_live_canonical_skill_activation_after_snapshot() -> Result<()> {
        use crate::state::RuntimeStateStore;

        let temp = tempfile::tempdir()?;
        let store = Arc::new(crate::state::FileRuntimeStateStore::new(temp.path())?);
        let config = crate::agent::AgentConfig::new("test-model", "agent", "system")
            .conversation_id("skill-authority");
        let mut agent = crate::agent::ReactAgent::new(config);
        agent.set_state_store(store.clone());
        assert!(agent.tools.skill_registry.mark_activated("before-snapshot"));
        let snapshot = AgentRunSnapshot::from_agent(&agent);

        agent.tools.skill_registry.reset_activation_state();
        assert!(agent.tools.skill_registry.mark_activated("after-snapshot"));
        snapshot
            .save_runtime_checkpoint(&agent.memory.context, None)
            .await?;
        let checkpoint = store
            .get_checkpoint("skill-authority")
            .await?
            .ok_or_else(|| ReactError::Other("skill checkpoint missing".to_string()))?;
        assert_eq!(checkpoint.active_skills, vec!["after-snapshot".to_string()]);

        agent.tools.skill_registry.reset_activation_state();
        snapshot
            .save_runtime_checkpoint(&agent.memory.context, None)
            .await?;
        let reset_checkpoint = store
            .get_checkpoint("skill-authority")
            .await?
            .ok_or_else(|| ReactError::Other("reset checkpoint missing".to_string()))?;
        assert!(reset_checkpoint.active_skills.is_empty());
        Ok(())
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
                resource_guards: Vec::new(),
                subagent_lineage: None,
                uplink: None,
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
    async fn runtime_cursor_is_stable_across_safe_points_and_compaction() -> Result<()> {
        let new = crate::memory::project_messages(
            "product-conversation",
            &[
                Message::user("again".to_string()),
                Message::assistant("same answer".to_string()),
            ],
        )?;
        let mut cursor = TranscriptProjectionCursor::default();
        let assigned = cursor.assign("new-runtime-incarnation", &new)?;
        cursor.projected = assigned.clone();
        let same_safe_point = cursor.assign("new-runtime-incarnation", &new)?;
        assert_eq!(same_safe_point, assigned);

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
        assert_eq!(
            after_compact.first().map(|message| message.ordinal),
            Some(0)
        );
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
        context.apply_projections(&[ContextProjection::replacing(
            "provider-marker",
            Some(Message::user("internal projected state".to_string())),
        )]);
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
        let lineage = echo_core::tools::SubagentLineage {
            agent_name: Some("child-atomic".into()),
            run_id: Some("run-atomic".into()),
            execution_id: Some("execution-atomic".into()),
            ..Default::default()
        };
        let uplink: echo_core::tools::SubagentUplinkFn = Arc::new(|_| {
            Box::pin(async {
                echo_core::tools::SubagentUplinkReceipt {
                    accepted: true,
                    status: "observed".into(),
                    detail: String::new(),
                }
            })
        });
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
                subagent_lineage: Some(lineage.clone()),
                uplink: Some(uplink.clone()),
            }),
            working_dir: Some(std::path::PathBuf::from("/tmp/worktree-atomic")),
            cancel: None,
            disabled_tools: None,
            allowed_tools: None,
            tool_access: None,
            visible_tools: None,
            run_budget: None,
            history: None,
            resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                "invocation-guard".to_string(),
            )],
            input_lifecycle: None,
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
        assert_eq!(snapshot.subagent_lineage.as_ref(), Some(&lineage));
        assert!(
            snapshot
                .external_uplink
                .as_ref()
                .is_some_and(|value| Arc::ptr_eq(value, &uplink))
        );
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
        // This tests model-profile canonical text, independent of the checkout's
        // optional project-rules feature and local instruction files.
        let config = crate::agent::AgentConfig::new("model", "assistant", "Base prompt")
            .enable_tool(false)
            .auto_project_rules(false)
            .model_profile(profile)
            .token_limit(64);
        let agent = crate::agent::ReactAgent::new(config);

        let mut context = agent.memory.context.lock().await;
        // Full-feature prompts can exceed the old 64-token fixture. Exercise
        // canonical retention within a valid budget, with real history eviction.
        let input_limit = context.token_estimate().saturating_add(64);
        context.set_token_limit(input_limit, None);
        context.push(echo_core::llm::types::Message::user(
            "old context ".repeat(200),
        ));
        context.push(echo_core::llm::types::Message::user(
            "temporary context".to_string(),
        ));
        let (stats, _) = context.force_compress(1).await?;
        assert!(stats.evicted > 0);
        assert!(stats.after_tokens <= input_limit);
        assert!(context.messages().iter().any(|message| {
            message.text_content().is_some_and(|text| {
                text.contains("Base prompt") && text.contains("Use compact tool arguments.")
            })
        }));
        Ok(())
    }

    #[test]
    fn runtime_snapshot_bounds_malformed_context_window_facts() -> Result<()> {
        use echo_core::llm::capabilities::{
            ModelFactConfidence, ModelFactMetadata, ModelFactSet, ModelFactSource,
            ModelProfileOverride, ModelProfileResolver,
        };

        let resolution = ModelProfileResolver::new()
            .register_explicit_override(
                "custom",
                "future-model",
                ModelFactSet::new(
                    ModelFactMetadata::new(
                        ModelFactSource::CallerOverride,
                        "test:malformed-window",
                        "window-v1",
                        std::time::SystemTime::UNIX_EPOCH,
                        None,
                        ModelFactConfidence::VERIFIED,
                    ),
                    None,
                    ModelProfileOverride {
                        context_window: Some(u32::MAX),
                        ..Default::default()
                    },
                ),
            )
            .resolve_at("custom", "future-model", std::time::SystemTime::UNIX_EPOCH);
        let config = crate::agent::AgentConfig::new("future-model", "test", "system")
            .model_profile_resolution(resolution);

        let runtime = RuntimeConfig::from_agent_config(&config);
        assert_eq!(runtime.token_limit, 10_000_000);
        Ok(())
    }

    #[test]
    fn runtime_snapshot_uses_fresh_output_cap_when_unconfigured() -> Result<()> {
        use echo_core::llm::capabilities::{
            ModelFactConfidence, ModelFactMetadata, ModelFactSet, ModelFactSource,
            ModelProfileOverride, ModelProfileResolver,
        };

        let resolution = ModelProfileResolver::new()
            .register_explicit_override(
                "custom",
                "future-model",
                ModelFactSet::new(
                    ModelFactMetadata::new(
                        ModelFactSource::ExactModel,
                        "test:model-output-cap",
                        "output-v2",
                        std::time::SystemTime::UNIX_EPOCH,
                        None,
                        ModelFactConfidence::VERIFIED,
                    ),
                    None,
                    ModelProfileOverride {
                        max_output_tokens: Some(4_096),
                        ..Default::default()
                    },
                ),
            )
            .resolve_at("custom", "future-model", std::time::SystemTime::UNIX_EPOCH);
        let config = crate::agent::AgentConfig::new("future-model", "test", "system")
            .model_profile_resolution(resolution);

        let runtime = RuntimeConfig::from_agent_config(&config);
        assert_eq!(runtime.max_tokens, Some(4_096));
        Ok(())
    }

    #[test]
    fn snapshot_refreshes_retained_facts_without_an_llm_client() -> Result<()> {
        use echo_core::llm::capabilities::{
            ModelFactConfidence, ModelFactMetadata, ModelFactSet, ModelFactSource,
            ModelProfileOverride, ModelProfileResolver,
        };

        let resolution = ModelProfileResolver::new()
            .register_exact_model_facts(
                "openai",
                "future-model",
                ModelFactSet::new(
                    ModelFactMetadata::new(
                        ModelFactSource::ExactModel,
                        "test:expiring-model",
                        "model-v1",
                        std::time::SystemTime::UNIX_EPOCH,
                        Some(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1)),
                        ModelFactConfidence::VERIFIED,
                    ),
                    None,
                    ModelProfileOverride {
                        context_window: Some(32_000),
                        max_output_tokens: Some(4_096),
                        ..Default::default()
                    },
                ),
            )
            .resolve_at("openai", "future-model", std::time::SystemTime::UNIX_EPOCH);
        let agent = crate::agent::ReactAgent::new(
            crate::agent::AgentConfig::new("future-model", "test", "system")
                .model_profile_resolution(resolution),
        );

        let snapshot = AgentRunSnapshot::from_agent(&agent);
        assert_eq!(
            snapshot.config.token_limit,
            crate::agent::config::DEFAULT_TOKEN_LIMIT
        );
        assert_eq!(snapshot.config.max_tokens, None);
        assert!(
            snapshot
                .config
                .ignored_model_facts
                .iter()
                .any(|metadata| metadata.version == "model-v1")
        );
        Ok(())
    }

    #[test]
    fn tool_visibility_combines_skill_plan_and_disabled_policies() {
        let manager = Arc::new(crate::tools::ToolManager::new());
        manager.register(Box::new(ReadOnlyNamedTool("read_file")));
        for name in [
            "write_file",
            "shell",
            "final_answer",
            "custom",
            "mcp__malicious__write",
        ] {
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
                "mcp__malicious__write".to_string(),
            ])),
            disabled_tools: HashSet::from(["final_answer".to_string()]),
            visibility: None,
            plan_mode: true,
            readonly_tools: false,
            #[cfg(feature = "human-loop")]
            permission_service: None,
        };

        let visible: Vec<String> = runtime
            .tools_for_llm()
            .into_iter()
            .map(|tool| tool.function.name)
            .collect();

        assert_eq!(visible, vec!["read_file"]);
    }

    struct InvocationAccessProbe {
        name: &'static str,
        readonly: bool,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Tool for InvocationAccessProbe {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "invocation access probe"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object","properties":{}})
        }
        fn risk_level(&self) -> echo_core::tools::ToolRiskLevel {
            if self.readonly {
                echo_core::tools::ToolRiskLevel::ReadOnly
            } else {
                echo_core::tools::ToolRiskLevel::Standard
            }
        }
        fn execute(&self, _: ToolParameters) -> futures::future::BoxFuture<'_, Result<ToolResult>> {
            Box::pin(async move {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(ToolResult::success("called"))
            })
        }
    }

    #[tokio::test]
    async fn readonly_invocation_snapshot_limits_schema_search_and_forced_calls() -> Result<()> {
        for hard_scope in [false, true] {
            let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let agent = crate::agent::ReactAgentBuilder::new()
                .model("test-model")
                .tool(Box::new(InvocationAccessProbe {
                    name: "read_file",
                    readonly: true,
                    calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                }))
                .tool(Box::new(InvocationAccessProbe {
                    name: "write_file",
                    readonly: false,
                    calls: Arc::clone(&writes),
                }))
                .build()?;
            let invocation = echo_core::agent::AgentInvocationContext {
                tool_access: Some(echo_core::tools::ToolAccess::ReadOnly),
                allowed_tools: hard_scope.then(|| {
                    HashSet::from([
                        "read_file".into(),
                        "write_file".into(),
                        "tool_search".into(),
                        "final_answer".into(),
                    ])
                }),
                ..Default::default()
            };
            let snapshot = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation);
            assert!(snapshot.tools.readonly_tools);
            let names = tool_names(&snapshot);
            assert!(names.iter().any(|name| name == "read_file"));
            assert!(!names.iter().any(|name| name == "write_file"));
            let visibility = snapshot.tools.visibility.as_ref().ok_or_else(|| {
                ReactError::Other("readonly invocation visibility missing".into())
            })?;
            assert!(
                !visibility
                    .available_names()
                    .iter()
                    .any(|name| name == "write_file")
            );
            let search = snapshot
                .tools
                .tool_manager
                .get_tool("tool_search")
                .ok_or_else(|| ReactError::Other("tool_search missing".into()))?;
            search
                .execute_with_context(
                    [("query".into(), serde_json::json!("write_file"))]
                        .into_iter()
                        .collect(),
                    &echo_core::tools::ToolContext {
                        tool_access: Some(echo_core::tools::ToolAccess::ReadOnly),
                        tool_visibility: Some(Arc::clone(visibility)),
                        ..Default::default()
                    },
                )
                .await?;
            assert!(!visibility.is_eligible("write_file"));
            assert!(!visibility.is_visible("write_file"));
            let denied = snapshot
                .execute_tool_with_policy(
                    "readonly-forced".into(),
                    "write_file",
                    &ToolParameters::new(),
                    &serde_json::json!({}),
                    None,
                )
                .await;
            assert!(denied.is_err());
            assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 0);
            assert!(
                !agent.config.readonly_tools,
                "test must use invocation-only restriction"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn readonly_invocation_rechecks_same_name_and_late_mutating_registrations() -> Result<()>
    {
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .tool(Box::new(InvocationAccessProbe {
                name: "replaceable",
                readonly: true,
                calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }))
            .build()?;
        let snapshot = AgentRunSnapshot::from_agent_with_invocation(
            &agent,
            &echo_core::agent::AgentInvocationContext {
                tool_access: Some(echo_core::tools::ToolAccess::ReadOnly),
                ..Default::default()
            },
        );
        assert!(
            tool_names(&snapshot)
                .iter()
                .any(|name| name == "replaceable")
        );
        snapshot
            .tools
            .tool_manager
            .replace(Box::new(InvocationAccessProbe {
                name: "replaceable",
                readonly: false,
                calls: Arc::clone(&writes),
            }));
        snapshot
            .tools
            .tool_manager
            .register(Box::new(InvocationAccessProbe {
                name: "late_write",
                readonly: false,
                calls: Arc::clone(&writes),
            }));
        let search = snapshot
            .tools
            .tool_manager
            .get_tool("tool_search")
            .ok_or_else(|| ReactError::Other("tool_search missing".into()))?;
        let searched = search
            .execute_with_context(
                [("query".into(), serde_json::json!("replaceable"))]
                    .into_iter()
                    .collect(),
                &echo_core::tools::ToolContext {
                    tool_access: Some(echo_core::tools::ToolAccess::ReadOnly),
                    tool_visibility: snapshot.tools.visibility.clone(),
                    ..Default::default()
                },
            )
            .await?;
        assert!(
            !searched.metadata.contains_key("matched_tools"),
            "search rediscovered a same-name Mutating replacement: {}",
            searched.output
        );
        for name in ["replaceable", "late_write"] {
            assert!(!tool_names(&snapshot).iter().any(|visible| visible == name));
            let denied = snapshot
                .execute_tool_with_policy(
                    format!("forced-{name}"),
                    name,
                    &ToolParameters::new(),
                    &serde_json::json!({}),
                    None,
                )
                .await;
            assert!(denied.is_err());
        }
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn readonly_config_cannot_be_promoted_and_none_keeps_writer_behavior() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .readonly_tools()
            .build()?;
        let snapshot = AgentRunSnapshot::from_agent_with_invocation(
            &agent,
            &echo_core::agent::AgentInvocationContext {
                tool_access: Some(echo_core::tools::ToolAccess::Mutating),
                ..Default::default()
            },
        );
        assert!(snapshot.tools.readonly_tools);
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let writer = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .tool(Box::new(InvocationAccessProbe {
                name: "write_probe",
                readonly: false,
                calls: Arc::clone(&writes),
            }))
            .build()?;
        let snapshot = AgentRunSnapshot::from_agent_with_invocation(&writer, &Default::default());
        assert!(!snapshot.tools.readonly_tools);
        let result = snapshot
            .execute_tool_with_policy(
                "writer-control".into(),
                "write_probe",
                &ToolParameters::new(),
                &serde_json::json!({}),
                None,
            )
            .await;
        assert!(result.is_ok());
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 1);
        Ok(())
    }

    #[cfg(feature = "subagent")]
    #[tokio::test]
    async fn readonly_agent_dispatch_schema_preserves_legitimate_readonly_delegation() -> Result<()>
    {
        use crate::agent::subagent::{SubagentAccessMode, SubagentDefinition, SubagentRegistry};

        let registry = Arc::new(SubagentRegistry::new());
        let mut reader = SubagentDefinition::new("reader", "只读研究");
        reader.access_mode = SubagentAccessMode::ReadOnly;
        registry
            .register(reader, Box::new(crate::testing::MockAgent::new("reader")))
            .await;
        registry
            .register(
                SubagentDefinition::new("writer", "写入角色"),
                Box::new(crate::testing::MockAgent::new("writer")),
            )
            .await;
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .readonly_tools()
            .subagent_registry(registry)
            .register_agent_dispatch_tool()
            .build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let schema = snapshot
            .tools
            .tools_for_llm()
            .into_iter()
            .find(|tool| tool.function.name == "agent_tool")
            .ok_or_else(|| {
                ReactError::Other("readonly invocation lost legitimate agent_tool schema".into())
            })?;
        let targets = schema
            .function
            .parameters
            .pointer("/properties/agent_name/enum")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| ReactError::Other("readonly dispatch target schema missing".into()))?;
        assert_eq!(targets, &vec![serde_json::json!("reader")]);
        Ok(())
    }

    #[tokio::test]
    async fn readonly_tools_hides_and_blocks_custom_mutation() -> Result<()> {
        let mut agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .readonly_tools()
            .tool(Box::new(ReadOnlyNamedTool("custom_read")))
            .tool(Box::new(NamedTool("custom_write")))
            .build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        assert!(tool_names(&snapshot).contains(&"custom_read".to_string()));
        assert!(!tool_names(&snapshot).contains(&"custom_write".to_string()));
        assert!(!agent.tool_names().contains(&"custom_write".to_string()));

        agent.add_tools(vec![
            Box::new(ReadOnlyNamedTool("batch_read")),
            Box::new(NamedTool("batch_write")),
        ]);
        assert!(agent.tool_names().contains(&"batch_read".to_string()));
        assert!(!agent.tool_names().contains(&"batch_write".to_string()));
        agent.add_tool(Box::new(ReadOnlyNamedTool("replaceable")));
        assert!(
            agent
                .replace_tool(Box::new(NamedTool("replaceable")))
                .is_none()
        );
        assert!(
            agent
                .tool_manager()
                .get_tool("replaceable")
                .is_some_and(|tool| tool.capabilities().is_read_only())
        );

        agent.add_tool(Box::new(NamedTool("late_write")));
        assert!(!agent.tool_names().contains(&"late_write".to_string()));
        echo_core::agent::Agent::register_tool(&agent, Box::new(NamedTool("trait_write")));
        assert!(!agent.tool_names().contains(&"trait_write".to_string()));
        agent
            .tool_manager()
            .register(Box::new(NamedTool("injected_write")));
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        assert!(!tool_names(&snapshot).contains(&"late_write".to_string()));
        assert!(!tool_names(&snapshot).contains(&"injected_write".to_string()));
        let result = snapshot
            .execute_tool_with_policy(
                "readonly-custom-write".to_string(),
                "injected_write",
                &ToolParameters::new(),
                &serde_json::json!({}),
                None,
            )
            .await;
        let Err(failure) = result else {
            return Err(ReactError::Other(
                "readonly agent executed a late custom write tool".to_string(),
            ));
        };
        assert!(
            failure
                .result
                .as_ref()
                .ok_or_else(|| crate::error::ReactError::Other(
                    "expected captured ToolResult".to_string()
                ))?
                .error
                .as_deref()
                .is_some_and(|reason| reason.contains("read-only"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn readonly_agent_keeps_task_list_and_blocks_effectful_tools() -> Result<()> {
        let store: Arc<dyn crate::memory::Store> = Arc::new(crate::memory::InMemoryStore::new());
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .readonly_tools()
            .store(store)
            .build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let visible = tool_names(&snapshot);
        assert!(visible.contains(&"task_list".to_string()));
        let result = snapshot
            .execute_tool_with_policy(
                "readonly-task-list".to_string(),
                "task_list",
                &ToolParameters::new(),
                &serde_json::json!({}),
                None,
            )
            .await;
        let Err(failure) = result else {
            return Err(ReactError::Other(
                "empty task graph unexpectedly returned tasks".to_string(),
            ));
        };
        assert!(
            failure
                .result
                .as_ref()
                .ok_or_else(|| crate::error::ReactError::Other(
                    "expected captured ToolResult".to_string()
                ))?
                .error
                .as_deref()
                .is_some_and(|reason| reason.contains("No tasks"))
        );

        for name in ["task_create", "task_update", "recall", "search_memory"] {
            assert!(!visible.contains(&name.to_string()), "{name} was visible");
            let input = if matches!(name, "recall" | "search_memory") {
                serde_json::json!({"query": "missing"})
            } else {
                serde_json::json!({})
            };
            let params = input
                .as_object()
                .map(|map| {
                    map.iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect()
                })
                .unwrap_or_default();
            let result = snapshot
                .execute_tool_with_policy(format!("readonly-{name}"), name, &params, &input, None)
                .await;
            let Err(failure) = result else {
                return Err(ReactError::Other(format!("{name} executed")));
            };
            assert!(
                failure
                    .result
                    .as_ref()
                    .ok_or_else(|| crate::error::ReactError::Other(
                        "expected captured ToolResult".to_string()
                    ))?
                    .error
                    .as_deref()
                    .is_some_and(|reason| reason.contains("read-only"))
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn readonly_agent_keeps_cell_list_and_blocks_stop() -> Result<()> {
        let registry = Arc::new(
            echo_orchestration::tasks::BackgroundCommandManager::new(
                echo_orchestration::tasks::BackgroundCommandManagerConfig::default(),
            )
            .map_err(ReactError::Other)?,
        );
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .readonly_tools()
            .command_cells(registry)
            .build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let visible = tool_names(&snapshot);
        assert!(visible.contains(&"list_cells".to_string()));
        assert!(!visible.contains(&"stop_cell".to_string()));
        let result = snapshot
            .execute_tool_with_policy(
                "readonly-list-cells".to_string(),
                "list_cells",
                &ToolParameters::new(),
                &serde_json::json!({}),
                None,
            )
            .await;
        assert!(result.is_ok());
        Ok(())
    }

    #[cfg(feature = "subagent")]
    #[tokio::test]
    async fn readonly_agent_keeps_subagent_list_and_blocks_message() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .readonly_tools()
            .register_subagent_message_tools()
            .build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let visible = tool_names(&snapshot);
        assert!(visible.contains(&"subagent_list".to_string()));
        assert!(!visible.contains(&"subagent_message".to_string()));
        let result = snapshot
            .execute_tool_with_policy(
                "readonly-subagent-list".to_string(),
                "subagent_list",
                &ToolParameters::new(),
                &serde_json::json!({}),
                None,
            )
            .await;
        assert!(result.is_ok());
        Ok(())
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn permission_plan_hides_and_blocks_locally_mutating_mcp_tool() -> Result<()> {
        let tool_name = crate::mcp::McpToolAdapter::exposed_name_for("malicious", "write");
        let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut agent = crate::agent::ReactAgent::new(
            crate::agent::AgentConfig::new("test-model", "agent", "system")
                .permission_mode(echo_core::tools::permission::PermissionMode::Plan),
        );
        agent.add_tool(Box::new(McpPlanProbe {
            name: tool_name.clone(),
            executions: Arc::clone(&executions),
        }));

        let snapshot = AgentRunSnapshot::from_agent(&agent);
        assert!(snapshot.config.plan_mode);
        assert!(!tool_names(&snapshot).contains(&tool_name));

        let input = serde_json::json!({});
        let result = snapshot
            .execute_tool_with_policy(
                "call-mcp-plan".to_string(),
                &tool_name,
                &ToolParameters::new(),
                &input,
                None,
            )
            .await;
        let Err(failure) = result else {
            return Err(echo_core::error::ReactError::Other(
                "mutating MCP tool executed in permission plan mode".to_string(),
            ));
        };
        assert!(
            failure
                .result
                .as_ref()
                .ok_or_else(|| crate::error::ReactError::Other(
                    "expected captured ToolResult".to_string()
                ))?
                .error
                .as_deref()
                .is_some_and(|reason| reason.contains("Plan mode"))
        );
        assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 0);
        Ok(())
    }

    #[cfg(all(feature = "mcp", feature = "human-loop"))]
    #[tokio::test]
    async fn live_permission_plan_precedes_hook_allow_for_mutating_mcp_tool() -> Result<()> {
        use crate::skills::hooks::{HookAction, HookEvent, HookRule, HooksDefinition};

        let tool_name = crate::mcp::McpToolAdapter::exposed_name_for("malicious", "write");
        let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let service = Arc::new(
            crate::human_loop::PermissionService::new()
                .with_mode(echo_core::tools::permission::PermissionMode::Default),
        );
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .permission_service(Arc::clone(&service))
            .tool(Box::new(McpPlanProbe {
                name: tool_name.clone(),
                executions: Arc::clone(&executions),
            }))
            .build()?;
        let mut hooks = HooksDefinition::default();
        hooks.add_rules(
            HookEvent::PermissionRequest,
            vec![HookRule {
                matcher: "*".to_string(),
                hooks: vec![HookAction::Permission {
                    decision: "allow".to_string(),
                    reason: None,
                    suggestions: Vec::new(),
                }],
            }],
        );
        agent
            .hook_registry()
            .write()
            .await
            .register_user_hooks(hooks);

        let snapshot = AgentRunSnapshot::from_agent(&agent);
        assert!(!snapshot.config.plan_mode);
        service
            .set_mode(echo_core::tools::permission::PermissionMode::Plan)
            .await;
        assert!(snapshot.tools.is_plan_mode());
        assert!(!tool_names(&snapshot).contains(&tool_name));

        let input = serde_json::json!({});
        let result = snapshot
            .execute_tool_with_policy(
                "call-mcp-live-plan".to_string(),
                &tool_name,
                &ToolParameters::new(),
                &input,
                None,
            )
            .await;
        let Err(failure) = result else {
            return Err(echo_core::error::ReactError::Other(
                "hook allow bypassed live permission Plan mode".to_string(),
            ));
        };
        assert!(
            failure
                .result
                .as_ref()
                .ok_or_else(|| crate::error::ReactError::Other(
                    "expected captured ToolResult".to_string()
                ))?
                .error
                .as_deref()
                .is_some_and(|reason| reason.contains("Plan mode"))
        );
        assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 0);
        Ok(())
    }

    #[cfg(all(feature = "mcp", feature = "human-loop"))]
    #[tokio::test]
    async fn live_plan_mode_is_rechecked_after_call_scoped_allow() -> Result<()> {
        use crate::skills::hooks::HookEvent;
        use echo_core::hooks::HookResult;

        let tool_name = crate::mcp::McpToolAdapter::exposed_name_for("malicious", "write");
        let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let service = Arc::new(
            crate::human_loop::PermissionService::new()
                .with_mode(echo_core::tools::permission::PermissionMode::Default),
        );
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .permission_service(Arc::clone(&service))
            .tool(Box::new(McpPlanProbe {
                name: tool_name.clone(),
                executions: Arc::clone(&executions),
            }))
            .build()?;
        let hook_entered = Arc::new(tokio::sync::Notify::new());
        let hook_release = Arc::new(tokio::sync::Notify::new());
        let entered = Arc::clone(&hook_entered);
        let release = Arc::clone(&hook_release);
        agent.hook_registry().write().await.set_programmatic_hook(
            "live-plan-race",
            &[HookEvent::PreToolUse],
            Arc::new(move |_context| {
                let entered = Arc::clone(&entered);
                let release = Arc::clone(&release);
                Box::pin(async move {
                    entered.notify_one();
                    release.notified().await;
                    HookResult::allow()
                })
            }),
        );
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let call_snapshot = snapshot.clone();
        let call = tokio::spawn(async move {
            call_snapshot
                .execute_tool_with_policy(
                    "call-live-plan-race".to_string(),
                    &tool_name,
                    &ToolParameters::new(),
                    &serde_json::json!({}),
                    None,
                )
                .await
        });
        hook_entered.notified().await;
        service
            .set_mode(echo_core::tools::permission::PermissionMode::Plan)
            .await;
        hook_release.notify_one();
        let failure = call
            .await
            .map_err(|error| ReactError::Other(format!("live Plan call task failed: {error}")))?
            .err()
            .ok_or_else(|| ReactError::Other("stale Allow bypassed live Plan mode".to_string()))?;
        assert!(
            failure
                .result
                .as_ref()
                .ok_or_else(|| crate::error::ReactError::Other(
                    "expected captured ToolResult".to_string()
                ))?
                .error
                .as_deref()
                .is_some_and(|reason| reason.contains("Plan mode"))
        );
        assert_eq!(
            failure
                .result
                .as_ref()
                .ok_or_else(|| crate::error::ReactError::Other(
                    "expected captured ToolResult".to_string()
                ))?
                .failure
                .as_ref()
                .map(|failure| failure.category),
            Some(crate::tools::ToolFailureCategory::Unavailable)
        );
        assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 0);
        Ok(())
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn readonly_surface_is_rechecked_after_tool_replacement() -> Result<()> {
        let tool_name = "replaceable_readonly".to_string();
        let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .readonly_tools()
            .tool(Box::new(ReadOnlyNamedTool("replaceable_readonly")))
            .build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let mut ctx = ToolExecutionContext {
            call_id: "call-readonly-replacement".to_string(),
            requested_tool_name: tool_name.clone(),
            requested_input: serde_json::json!({}),
            tool_name: tool_name.clone(),
            params: ToolParameters::new(),
            input: serde_json::json!({}),
            hook_messages: HookMessageBatches::default(),
            result: None,
            output: None,
            audit_error_output: None,
            blocked: false,
            block_reason: None,
            block_failure: None,
            duration_ms: 0,
            plan_mode: false,
            permission_decision: None,
            approval_receipt: None,
            authorized_tool: None,
            permission_mode_override: None,
            rewrites: Vec::new(),
            invocation_emitted: false,
            callback_started: false,
            execution_started: false,
            interrupted_execution_error: None,
            stream_tx: None,
        };

        PlanModeStage.run(&mut ctx, &snapshot).await?;
        assert!(!ctx.blocked);
        snapshot.tools.tool_manager.replace(Box::new(McpPlanProbe {
            name: tool_name,
            executions: Arc::clone(&executions),
        }));

        // 本独立stage控制在replace后绑定B，继续验证原physical readonly拒绝。
        ctx.authorized_tool = snapshot.tools.tool_manager.get_tool(&ctx.tool_name);
        ExecuteStage.run(&mut ctx, &snapshot).await?;
        assert!(ctx.blocked);
        assert_eq!(
            ctx.block_reason.as_deref(),
            Some("Tool 'replaceable_readonly' is blocked by the read-only Agent")
        );
        assert_eq!(
            ctx.block_failure.as_ref().map(|failure| failure.category),
            Some(crate::tools::ToolFailureCategory::Unavailable)
        );
        assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 0);
        Ok(())
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

    #[tokio::test]
    async fn inherited_hard_scope_cannot_be_promoted_by_tool_search() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .tool(Box::new(NamedTool("read_file")))
            .tool(Box::new(NamedTool("write_file")))
            .build()?;
        let invocation = echo_core::agent::AgentInvocationContext {
            allowed_tools: Some(HashSet::from(["read_file".into(), "tool_search".into()])),
            visible_tools: Some(HashSet::from(["read_file".into(), "tool_search".into()])),
            ..Default::default()
        };
        let snapshot = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation);
        let visibility = snapshot
            .tools
            .visibility
            .as_ref()
            .ok_or_else(|| crate::error::ReactError::Other("visibility missing".into()))?;
        assert!(
            !visibility
                .available_names()
                .iter()
                .any(|name| name == "write_file")
        );
        assert!(
            visibility
                .extend_eligibility_and_activate(["write_file".into()])
                .is_empty()
        );
        let search = snapshot
            .tools
            .tool_manager
            .get_tool("tool_search")
            .ok_or_else(|| crate::error::ReactError::Other("tool_search missing".into()))?;
        search
            .execute_with_context(
                [("query".into(), serde_json::json!("write_file"))]
                    .into_iter()
                    .collect(),
                &echo_core::tools::ToolContext {
                    tool_visibility: Some(Arc::clone(visibility)),
                    ..Default::default()
                },
            )
            .await?;
        assert!(!visibility.is_eligible("write_file"));
        assert!(!visibility.is_visible("write_file"));
        assert!(
            !tool_names(&snapshot)
                .iter()
                .any(|name| name == "write_file")
        );
        assert!(agent.tool_names().iter().any(|name| name == "write_file"));
        Ok(())
    }

    #[test]
    fn hard_tool_scope_does_not_confuse_initial_visibility_with_permission() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .tool(Box::new(NamedTool("read_file")))
            .tool(Box::new(NamedTool("write_file")))
            .build()?;
        let invocation = echo_core::agent::AgentInvocationContext {
            allowed_tools: Some(HashSet::from(["read_file".into(), "tool_search".into()])),
            visible_tools: Some(HashSet::from(["tool_search".into()])),
            ..Default::default()
        };
        let snapshot = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation);
        let visibility = snapshot
            .tools
            .visibility
            .as_ref()
            .ok_or_else(|| crate::error::ReactError::Other("visibility missing".into()))?;
        assert!(visibility.is_eligible("read_file"));
        assert!(!visibility.is_visible("read_file"));
        assert_eq!(visibility.activate(["read_file".into()]), vec!["read_file"]);
        assert!(!visibility.is_eligible("write_file"));
        assert!(snapshot.tools.disabled_tools.contains("write_file"));
        let initial_all = AgentRunSnapshot::from_agent_with_invocation(
            &agent,
            &echo_core::agent::AgentInvocationContext {
                allowed_tools: invocation.allowed_tools,
                ..Default::default()
            },
        );
        assert!(
            tool_names(&initial_all)
                .iter()
                .any(|name| name == "read_file")
        );
        assert!(
            !tool_names(&initial_all)
                .iter()
                .any(|name| name == "write_file")
        );
        Ok(())
    }

    #[test]
    fn empty_hard_tool_scope_retains_legacy_no_extra_whitelist() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .tool(Box::new(NamedTool("read_file")))
            .build()?;
        let plain = AgentRunSnapshot::from_agent_with_invocation(
            &agent,
            &echo_core::agent::AgentInvocationContext::default(),
        );
        let empty = AgentRunSnapshot::from_agent_with_invocation(
            &agent,
            &echo_core::agent::AgentInvocationContext {
                allowed_tools: Some(HashSet::new()),
                ..Default::default()
            },
        );
        assert_eq!(tool_names(&plain), tool_names(&empty));
        assert!(empty.tools.visibility.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn nondelegating_policy_qualifies_search_even_without_initial_visibility() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .tool(Box::new(NamedTool("agent_tool")))
            .tool(Box::new(NamedTool("read_file")))
            .build()?;
        let stopped = echo_core::tools::NestedDelegationPolicy {
            can_spawn_subagents: false,
            delegate_depth: 0,
            max_delegate_depth: 3,
        };
        for allowed_tools in [None, Some(HashSet::from(["agent_tool".to_string()]))] {
            let scope_only_dispatch = allowed_tools.is_some();
            let snapshot = AgentRunSnapshot::from_agent_with_invocation(
                &agent,
                &echo_core::agent::AgentInvocationContext {
                    runtime: Some(echo_core::tools::ExternalRunContext {
                        delegation_policy: Some(stopped),
                        ..Default::default()
                    }),
                    allowed_tools,
                    visible_tools: None,
                    ..Default::default()
                },
            );
            assert_eq!(snapshot.external_delegation_policy, Some(stopped));
            assert!(snapshot.tools.disabled_tools.contains("agent_tool"));
            assert!(
                !tool_names(&snapshot)
                    .iter()
                    .any(|name| name == "agent_tool")
            );
            let visibility = snapshot.tools.visibility.as_ref().ok_or_else(|| {
                crate::error::ReactError::Other("policy availability missing".into())
            })?;
            assert!(
                !visibility
                    .available_names()
                    .iter()
                    .any(|name| name == "agent_tool")
            );
            assert!(!visibility.is_eligible("agent_tool"));
            assert!(
                visibility
                    .extend_eligibility_and_activate(["agent_tool".into()])
                    .is_empty()
            );
            if scope_only_dispatch {
                assert!(visibility.available_names().is_empty());
                assert!(tool_names(&snapshot).is_empty());
            } else {
                assert!(
                    !tool_names(&snapshot)
                        .iter()
                        .any(|name| name == "tool_search")
                );
            }
            let search = snapshot
                .tools
                .tool_manager
                .get_tool("tool_search")
                .ok_or_else(|| crate::error::ReactError::Other("search missing".into()))?;
            let result = search
                .execute_with_context(
                    [("query".into(), serde_json::json!("agent_tool"))]
                        .into_iter()
                        .collect(),
                    &echo_core::tools::ToolContext {
                        tool_visibility: Some(Arc::clone(visibility)),
                        delegation_policy: Some(stopped),
                        ..Default::default()
                    },
                )
                .await?;
            assert!(
                result
                    .metadata
                    .get("matched_tools")
                    .is_none_or(|names| !names.split(',').any(|name| name == "agent_tool"))
            );
        }
        Ok(())
    }

    #[test]
    fn policy_snapshot_is_invocation_local_and_plain_call_has_no_policy() -> Result<()> {
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .tool(Box::new(NamedTool("agent_tool")))
            .build()?;
        let stopped = echo_core::tools::NestedDelegationPolicy {
            can_spawn_subagents: true,
            delegate_depth: 1,
            max_delegate_depth: 1,
        };
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime: Some(echo_core::tools::ExternalRunContext {
                delegation_policy: Some(stopped),
                ..Default::default()
            }),
            ..Default::default()
        };
        let restricted = AgentRunSnapshot::from_agent_with_invocation(&agent, &invocation);
        assert_eq!(restricted.external_delegation_policy, Some(stopped));
        assert!(
            !tool_names(&restricted)
                .iter()
                .any(|name| name == "agent_tool")
        );
        for plain in [
            AgentRunSnapshot::from_agent(&agent),
            AgentRunSnapshot::from_agent_with_invocation(&agent, &Default::default()),
        ] {
            assert!(plain.external_delegation_policy.is_none());
            assert!(plain.tools.visibility.is_none());
            assert!(tool_names(&plain).iter().any(|name| name == "agent_tool"));
        }
        assert_eq!(restricted.external_delegation_policy, Some(stopped));
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
