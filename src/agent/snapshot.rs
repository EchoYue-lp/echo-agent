//! Agent execution snapshot — captures agent state for `'static` streaming.
//!
//! [`AgentRunSnapshot`] replaces the old 33-field manual `AgentSnapshot` clone
//! in `stream_channel.rs` with a composition-based approach. Configuration,
//! tool runtime, and guard runtime are each wrapped in `Arc` so the snapshot
//! is cheap to clone and safe to move into a `tokio::spawn` future.

use crate::agent::AgentCallback;
use crate::agent::InterventionCallback;
use crate::audit::AuditLogger;
use crate::memory::checkpointer::Checkpointer;
use crate::memory::snapshot::SnapshotManager;
use crate::skills::hooks::HookRegistry;
use crate::tools::{ToolExecutionConfig, ToolManager};
use crate::trace::{RunEvent, RunStatus, RunStore};
use echo_core::circuit_breaker::CircuitBreaker;
use std::sync::Arc;

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
    pub audit_logger: Option<Arc<dyn AuditLogger>>,
    pub circuit_breaker: Option<Arc<CircuitBreaker>>,
}

impl GuardRuntime {
    pub fn from_agent(agent: &super::ReactAgent) -> Self {
        Self {
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
    /// Checkpointer (from memory subsystem).
    pub checkpointer: Option<Arc<dyn Checkpointer>>,
    /// Snapshot manager (from memory subsystem).
    pub snapshot_manager: Arc<std::sync::RwLock<Option<SnapshotManager>>>,
    /// HTTP client.
    pub client: Arc<reqwest::Client>,
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
    /// Runtime state store for rich checkpointing (messages + plan + skills).
    pub state_store: Option<Arc<dyn crate::state::RuntimeStateStore>>,
    /// Optional Critic for final_answer verification.
    pub critic: Option<Arc<dyn echo_core::agent::Critic>>,
}

impl AgentRunSnapshot {
    /// Create a snapshot from a [`ReactAgent`].
    pub fn from_agent(agent: &super::ReactAgent) -> Self {
        Self {
            config: Arc::new(RuntimeConfig::from_agent_config(&agent.config)),
            tools: Arc::new(ToolRuntime::from_agent(agent)),
            guard: Arc::new(GuardRuntime::from_agent(agent)),
            checkpointer: agent.memory.checkpointer.clone(),
            snapshot_manager: agent.memory.snapshot_manager.clone(),
            client: agent.client().clone(),
            cancel_token: None,
            recently_read_files: Arc::clone(&agent.recently_read_files),
            run_store: agent.run_store.clone(),
            current_run_id: None, // set by run_stream_channel
            #[cfg(feature = "human-loop")]
            permission_service: agent.approval.permission_service.clone(),
            token_tracker: Arc::clone(&agent.token_tracker),
            state_store: agent.state_store.clone(),
            critic: agent.critic.clone(),
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
    /// Unlike the legacy [`Checkpointer`](crate::memory::Checkpointer) which only
    /// persists message history, this saves the full [`AgentCheckpoint`] including
    /// messages, active skills, current plan, and blocked reason.
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

    // ── TaskNode DAG helpers ─────────────────────────────────────────

    /// Create a new TaskNode for the current execution turn.
    ///
    /// Returns the node ID if a state store is configured, `None` otherwise.
    pub async fn create_execution_node(&self, user_input: &str) -> Option<String> {
        let store = self.state_store.as_ref()?;
        let conv_id = self.config.conversation_id.as_ref()?;

        let node_id = format!("exec-{}", uuid::Uuid::new_v4());
        let name = if user_input.len() > 100 {
            format!("{}...", &user_input[..100])
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
}
