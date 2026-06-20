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
use crate::tools::{ToolExecutionConfig, ToolManager};
use crate::trace::{RunEvent, RunStatus, RunStore};
use echo_core::circuit_breaker::CircuitBreaker;
use echo_core::llm::types::{Message, Role};
use echo_core::tokenizer::Tokenizer;
use std::sync::Arc;

fn is_internal_transcript_message(message: &Message) -> bool {
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

// ── RuntimeConfig ────────────────────────────────────────────────────

/// Immutable subset of [`AgentConfig`](crate::agent::AgentConfig) that
/// does not change during a streaming run.
#[derive(Clone)]
pub struct RuntimeConfig {
    pub agent_name: String,
    pub model_name: String,
    pub max_iterations: usize,
    pub session_id: Option<String>,
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
}

impl RuntimeConfig {
    /// Create a snapshot from the agent's config.
    pub fn from_agent_config(config: &crate::agent::AgentConfig) -> Self {
        Self {
            agent_name: config.agent_name.clone(),
            model_name: config.model_name.clone(),
            max_iterations: config.max_iterations,
            session_id: config.session_id.clone(),
            conversation_id: config.conversation_id.clone(),
            working_dir: config.working_dir.lock().unwrap().clone(),
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            tool_error_feedback: config.tool_error_feedback,
            force_read_before_edit: config.force_read_before_edit,
            enable_tool: config.enable_tool,
            llm_max_retries: config.llm_max_retries,
            llm_retry_delay_ms: config.llm_retry_delay_ms,
            max_tool_output_tokens: config.max_tool_output_tokens,
            tool_execution: config.tool_execution.clone(),
            callbacks: config.callbacks.to_vec(),
            react_checkpoint_interval: config.react_checkpoint_interval,
            verifier_enabled: config.verifier_enabled,
            verifier_min_score: config.verifier_min_score,
            verifier_max_retries: config.verifier_max_retries,
            plan_mode: config.plan_mode,
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
}

impl ToolRuntime {
    pub fn from_agent(agent: &super::ReactAgent) -> Self {
        Self {
            tool_manager: Arc::clone(&agent.tools.tool_manager),
            hook_registry: agent.tools.hook_registry.clone(),
            intervention_callbacks: agent.tools.intervention_callbacks.clone(),
            skill_allowed_tools: agent.tools.skill_registry.active_skill_allowed_tools(),
            active_skill_names: agent.tools.skill_registry.activated_names(),
            plan_state: Arc::clone(&agent.plan_state),
        }
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

/// Captures everything the streaming loop needs from a [`ReactAgent`] without
/// holding a reference to the agent itself.
///
/// Uses composition via `Arc` for all subsystems — cloning is O(1).
#[derive(Clone)]
pub struct AgentRunSnapshot {
    /// Immutable runtime configuration.
    pub config: Arc<RuntimeConfig>,
    /// Tool execution state (tools, hooks).
    pub tools: Arc<ToolRuntime>,
    /// Guard / safety state.
    pub guard: Arc<GuardRuntime>,
    /// Snapshot manager (from memory subsystem).
    pub snapshot_manager: Arc<std::sync::RwLock<Option<SnapshotManager>>>,
    /// HTTP client.
    pub client: Arc<reqwest::Client>,
    /// Optional trait-level LLM client. When present, the streaming core loop
    /// (`create_llm_stream`) and `direct_answer_stream` route LLM calls through
    /// this trait object instead of the raw `client` + model-resolve path —
    /// enabling test doubles (MockLlmClient) to drive the full ReAct loop
    /// without `echo-agent-models.yaml`. Production agents inject a real
    /// `LlmClient` impl (OpenAiClient, …); when `None`, the legacy reqwest
    /// fallback path is used (behavior unchanged).
    pub llm_client: Option<Arc<dyn crate::llm::LlmClient>>,
    /// Per-agent thinking-depth config, propagated to the think phase and react
    /// loop so each LLM request carries the configured reasoning depth. `None`
    /// means "use the model's default" (no thinking field sent).
    pub thinking: Option<crate::llm::ThinkingConfig>,
    /// Cancellation token (set after construction).
    pub cancel_token: Option<crate::agent::CancellationToken>,
    /// Recently read files for read-before-edit enforcement (path → read instant).
    pub recently_read_files:
        Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    /// Run store for trace persistence.
    pub run_store: Option<Arc<dyn RunStore>>,
    /// Current run ID.
    pub current_run_id: Option<String>,
    /// Permission service (human-in-the-loop).
    #[cfg(feature = "human-loop")]
    pub permission_service: Option<Arc<crate::human_loop::PermissionService>>,
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
    /// projected messages at every finalization point so GUI/TUI history is
    /// always in sync with the running context — without each entry point
    /// having to re-implement the save logic.
    pub conversation_store: Option<Arc<dyn crate::memory::ConversationStore>>,
    /// Optional Critic for final_answer verification.
    pub critic: Option<Arc<dyn echo_core::agent::Critic>>,
    /// Optional tool execution pipeline (15-stage middleware).
    pub tool_execution_pipeline:
        Option<Arc<crate::agent::react::run::pipeline::ToolExecutionPipeline>>,
}

impl AgentRunSnapshot {
    /// Create a snapshot from a [`ReactAgent`].
    pub fn from_agent(agent: &super::ReactAgent) -> Self {
        Self {
            config: Arc::new(RuntimeConfig::from_agent_config(&agent.config)),
            tools: Arc::new(ToolRuntime::from_agent(agent)),
            guard: Arc::new(GuardRuntime::from_agent(agent)),
            snapshot_manager: agent.memory.snapshot_manager.clone(),
            client: agent.client().clone(),
            llm_client: agent.llm_client().cloned(),
            thinking: agent.thinking().cloned(),
            cancel_token: agent.cancel_token.try_lock().ok().and_then(|g| g.clone()),
            recently_read_files: Arc::clone(&agent.recently_read_files),
            run_store: agent.run_store.clone(),
            current_run_id: None, // set by run_stream_channel
            #[cfg(feature = "human-loop")]
            permission_service: agent.approval.permission_service.clone(),
            token_tracker: Arc::clone(&agent.token_tracker),
            calibrated_tokenizer: Arc::clone(&agent.calibrated_tokenizer),
            state_store: agent.memory.state_store.clone(),
            conversation_store: agent.memory.conversation_store.clone(),
            critic: agent.critic.clone(),
            tool_execution_pipeline: agent.tool_execution_pipeline.clone(),
        }
    }

    // ── Trace helpers ──────────────────────────────────────────────

    /// Record a trace event if a run store is attached.
    pub async fn record_event(&self, event: RunEvent) {
        if let Some(ref store) = self.run_store
            && let Some(ref run_id) = self.current_run_id
        {
            let _ = store.append_event(run_id, event).await;
        }
    }

    /// Finalize the current trace run (completed or failed).
    pub async fn finalize_run(&self, status: RunStatus, output: Option<&str>, error: Option<&str>) {
        if let Some(ref store) = self.run_store
            && let Some(ref run_id) = self.current_run_id
            && let Ok(Some(mut run)) = store.load(run_id).await
        {
            run.status = status;
            run.final_output = output.map(|s| s.to_string());
            run.error = error.map(|s| s.to_string());
            run.finished_at = Some(chrono::Utc::now());
            let _ = store.save(run).await;
        }
    }

    // ── Runtime state checkpoint ─────────────────────────────────────

    /// Save a rich checkpoint to the [`RuntimeStateStore`](crate::state::RuntimeStateStore).
    ///
    /// Persists the full [`AgentCheckpoint`] (messages, active skills, current
    /// plan, and blocked reason) so an in-flight conversation can resume
    /// across process restarts.
    ///
    /// Silently no-ops if no state store or conversation_id is configured.
    pub async fn save_runtime_checkpoint(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        blocked_reason: Option<String>,
    ) {
        let Some(ref store) = self.state_store else {
            return;
        };
        let Some(ref conv_id) = self.config.conversation_id else {
            return;
        };

        let messages = {
            let ctx = context.lock().await;
            ctx.messages().to_vec()
        };

        let messages_json = match serde_json::to_string(&messages) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to serialize messages for runtime checkpoint"
                );
                return;
            }
        };

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

        if let Err(e) = store.save_checkpoint(&checkpoint).await {
            tracing::warn!(
                error = %e,
                conversation_id = conv_id.as_str(),
                "Failed to save runtime checkpoint to state store"
            );
        } else {
            tracing::debug!(
                conversation_id = conv_id.as_str(),
                message_count = messages.len(),
                "Runtime checkpoint saved"
            );
        }
    }

    /// Save the user-visible transcript projection to the [`ConversationStore`](crate::memory::ConversationStore).
    ///
    /// Unlike `save_runtime_checkpoint` which serializes the full runtime
    /// `Message` list (including internal/tool hand-offs) for resume, this
    /// projects messages to [`StoredMessage`](crate::memory::StoredMessage) records and persists
    /// them via `ConversationStore::save_messages` — the same shape that
    /// the GUI/TUI history panes consume.
    ///
    /// This consolidates transcript persistence in the framework: previously,
    /// every product entry point (Tauri commands, TUI loop) had to call
    /// `save_messages` on its own. Now `run_core_loop` invokes this helper at
    /// finalization, and the product layer only handles conversation
    /// metadata (title / pinned / agent_type).
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

        if let Err(e) = store.save_messages(conv_id, &projected).await {
            tracing::warn!(
                error = %e,
                conversation_id = conv_id.as_str(),
                "Failed to save transcript projection to conversation store"
            );
        } else {
            tracing::debug!(
                conversation_id = conv_id.as_str(),
                message_count = projected.len(),
                "Transcript projection saved"
            );
        }
    }

    // ── TaskNode DAG helpers ─────────────────────────────────────────

    /// Create a new TaskNode for the current execution turn.
    ///
    /// Returns the node ID if a state store is configured, `None` otherwise.
    pub async fn create_execution_node(&self, user_input: &str) -> Option<String> {
        let store = self.state_store.as_ref()?;
        let conv_id = self.config.conversation_id.as_ref()?;

        let node_id = format!("exec-{}", uuid::Uuid::new_v4());
        let name = if user_input.chars().count() > 100 {
            format!("{}...", user_input.chars().take(100).collect::<String>())
        } else {
            user_input.to_string()
        };

        let node = crate::state::TaskNode::new(&node_id, &name)
            .with_status(crate::state::TaskNodeStatus::Running);

        if let Err(e) = store.save_node(conv_id, &node).await {
            tracing::warn!(error = %e, "Failed to create execution TaskNode");
            return None;
        }

        tracing::debug!(node_id = %node_id, "Created execution TaskNode (Running)");
        Some(node_id)
    }

    /// Update a TaskNode's status.
    pub async fn update_node_status(&self, node_id: &str, status: crate::state::TaskNodeStatus) {
        let Some(store) = self.state_store.as_ref() else {
            return;
        };
        let Some(conv_id) = self.config.conversation_id.as_ref() else {
            return;
        };

        if let Err(e) = store.update_status(conv_id, node_id, status.clone()).await {
            tracing::warn!(error = %e, node_id = %node_id, "Failed to update TaskNode status");
        } else {
            tracing::debug!(node_id = %node_id, status = ?status, "TaskNode status updated");
        }
    }

    /// On resume: set any `Running` TaskNodes to `Hydrated`.
    pub async fn hydrate_running_nodes(&self) {
        let Some(store) = self.state_store.as_ref() else {
            return;
        };
        let Some(conv_id) = self.config.conversation_id.as_ref() else {
            return;
        };

        let nodes = match store.load_nodes(conv_id).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to load TaskNodes for hydration");
                return;
            }
        };

        for node in &nodes {
            if node.status == crate::state::TaskNodeStatus::Running {
                if let Err(e) = store
                    .update_status(conv_id, &node.id, crate::state::TaskNodeStatus::Hydrated)
                    .await
                {
                    tracing::warn!(error = %e, node_id = %node.id, "Failed to hydrate TaskNode");
                } else {
                    tracing::debug!(node_id = %node.id, "Hydrated previously Running TaskNode");
                }
            }
        }
    }

    // ── Tool execution helpers (delegated from Pipeline stages) ─────

    /// Check tool approval via PermissionService.
    /// Returns modified input if approval modified the tool call, None otherwise.
    #[cfg(feature = "human-loop")]
    pub async fn check_tool_approval(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> std::result::Result<Option<serde_json::Value>, echo_core::error::ReactError> {
        if let Some(ref service) = self.permission_service {
            let decision = service.check(tool_name, input).await?;
            match decision {
                echo_core::tools::permission::PermissionDecision::Allow => Ok(None),
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
        _tool_name: &str,
        _input: &serde_json::Value,
    ) -> std::result::Result<Option<serde_json::Value>, echo_core::error::ReactError> {
        Ok(None)
    }

    /// Record a file read for read-before-edit enforcement.
    pub fn record_file_read(&self, path: &str) {
        let canonical = std::fs::canonicalize(path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string());
        let mut files = self
            .recently_read_files
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        files.insert(canonical, std::time::Instant::now());
    }

    /// Check tool output guard and return filtered output if modified.
    pub async fn check_tool_output_guard(&self, output: &str) -> Option<String> {
        // Secret scan: redact secrets from tool output before guard check
        if crate::security::contains_secrets(output) {
            let redacted = crate::security::redact_secrets(output);
            tracing::warn!(agent = %self.config.agent_name, "Secret detected in tool output; redacted");
            return Some(redacted);
        }
        let gm = self.guard.guard_manager.as_ref()?;
        use crate::guard::GuardDirection;
        let result = match gm.check_all(output, GuardDirection::Output).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(agent = %self.config.agent_name, error = %e, "Guard check failed, blocking output (fail-closed)");
                return Some(format!("Output content blocked: guard check error ({e})"));
            }
        };
        if let crate::guard::GuardResult::Block { reason } = &result {
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
        } else {
            None
        }
    }

    /// Truncate tool output based on token budget.
    ///
    /// Uses `HeuristicTokenizer` for accurate ASCII/CJK-aware token estimation,
    /// matching the tokenizer used by `execution.rs` and `ContextManager`.
    pub async fn truncate_tool_output(&self, output: String) -> String {
        // Only apply truncation when a max token limit is configured
        let Some(max_tokens) = self.config.max_tool_output_tokens else {
            return output;
        };

        let tokenizer = echo_core::tokenizer::HeuristicTokenizer;
        let estimated_tokens = tokenizer.count_tokens(&output);
        if estimated_tokens <= max_tokens {
            return output;
        }

        // Truncate with head + tail strategy
        let head_chars = max_tokens * 2; // ~50% of budget for head
        let tail_chars = max_tokens * 2; // ~50% of budget for tail

        if output.len() <= head_chars + tail_chars {
            return output;
        }

        // UTF-8 safe truncation: find char boundaries
        let head_end = output
            .char_indices()
            .take_while(|(i, _)| *i < head_chars)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        let tail_start = output
            .char_indices()
            .rev()
            .take_while(|(i, _)| *i >= output.len() - tail_chars)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(output.len());
        let head = &output[..head_end];
        let tail = &output[tail_start..];
        let notice = format!(
            "\n\n[... output truncated: ~{} tokens → {} tokens shown ...]\n\n",
            estimated_tokens, max_tokens
        );
        format!("{}{}{}", head, notice, tail)
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
        tool_name: &'a str,
        params: &'a crate::tools::ToolParameters,
        input: &'a serde_json::Value,
    ) -> futures::future::BoxFuture<'a, std::result::Result<String, crate::error::ReactError>> {
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
                call_id: String::new(),
                tool_name: tool_name.to_string(),
                params: params.clone(),
                input: input.clone(),
                hook_messages: crate::agent::react::run::context::HookMessageBatches::default(),
                result: None,
                output: None,
                blocked: false,
                block_reason: None,
                duration_ms: 0,
                plan_mode: self.config.plan_mode,
            };

            match pipeline.run(&mut ctx, self).await {
                Ok(()) => {
                    // Check if execution was blocked
                    if ctx.blocked {
                        let reason = ctx
                            .block_reason
                            .unwrap_or_else(|| format!("Tool {} blocked", tool_name));
                        return Ok(reason);
                    }

                    // Return the final output (after guard + truncation)
                    if let Some(output) = ctx.output {
                        Ok(output)
                    } else if let Some(result) = ctx.result {
                        Ok(result.output)
                    } else {
                        Err(crate::error::ReactError::Other(
                            "Pipeline completed without result".into(),
                        ))
                    }
                }
                Err(e) => Err(e),
            }
        })
    }
}
