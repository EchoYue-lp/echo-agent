//! ReactAgent capability configuration API
//!
//! - Tool registration (`add_tool` / `add_tools` / `add_need_appeal_tool`)
//! - Skill installation (`add_skill` / `add_skills` / `discover_skills` / `load_skills_from_dir`)
//! - MCP connections (`connect_mcp` / `load_mcp_from_file`)
//! - Subagent, compressor, callbacks, etc.

use super::ReactAgent;
#[cfg(feature = "subagent")]
use crate::agent::Agent;
use crate::agent::AgentHandle;
#[cfg(feature = "subagent")]
use crate::agent::subagent::{AgentFactory, SubagentDefinition};
use crate::compression::{
    CompressionCheckpoint, ContextCompressor, ContextManager, ForceCompressStats,
};
use crate::error::Result;
use crate::llm::{LlmClient, LlmConfig, ThinkingConfig};
#[cfg(feature = "mcp")]
use crate::mcp::McpServerEntry;
#[cfg(feature = "mcp")]
use crate::mcp::{McpClient, McpConfigFile, McpServerConfig};
use crate::skills::external::activate_tool::ActivateSkillTool;
use crate::skills::external::loader::{DiscoveryScope, SkillLoader};
use crate::skills::external::resource_tool::ReadSkillResourceTool;
use crate::skills::external::run_script_tool::RunSkillScriptTool;
use crate::skills::{Skill, SkillInfo};
use crate::tools::Tool;
use echo_core::agent::Critic;
use echo_core::budget::TokenBudget;
use std::sync::Arc;
use tokio::sync::{OwnedMutexGuard, RwLock};
use tracing::{info, warn};

const SKILL_CATALOG_PROJECTION: &str = "echo-agent:skill-catalog";

pub(crate) async fn project_skill_activation(
    context: &Arc<tokio::sync::Mutex<ContextManager>>,
    skill_name: &str,
    prompt_block: &str,
) {
    context.lock().await.replace_projection(
        format!("echo-agent:skill:{skill_name}"),
        Some(crate::llm::types::Message::system(prompt_block.to_string())),
    );
}

/// A validated token-limit update with exclusive ownership of the live context.
///
/// Holding this value prevents an agent turn from entering the context between
/// preparation and commit. Dropping it cancels the update without changing the
/// agent or its context.
pub struct PreparedTokenLimit {
    agent: tokio::sync::OwnedRwLockWriteGuard<ReactAgent>,
    values: PreparedTokenLimitValues,
}

struct PreparedTokenLimitValues {
    token_limit: usize,
    budget: Option<TokenBudget>,
    context: OwnedMutexGuard<ContextManager>,
}

/// Explicit critic publication policy for a prepared model generation.
pub enum PreparedCriticUpdate {
    /// Leave the currently installed critic and its ownership unchanged.
    Preserve,
    /// Replace the critic only when it is still owned by `owner`.
    ///
    /// A later [`ReactAgent::set_critic`] call clears named ownership, so a
    /// consumer cannot overwrite a user-installed custom critic accidentally.
    ReplaceOwned {
        owner: String,
        critic: Arc<dyn Critic>,
    },
}

/// A complete, prevalidated runtime model generation for a [`ReactAgent`].
///
/// The LLM client is constructed by the consumer before preparation, so commit
/// performs no I/O, configuration parsing, or provider lookup.
pub struct PreparedAgentModelGeneration {
    agent: tokio::sync::OwnedRwLockWriteGuard<ReactAgent>,
    llm_config: LlmConfig,
    llm_client: Arc<dyn LlmClient>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    thinking: Option<ThinkingConfig>,
    critic: PreparedCriticUpdate,
    token_limit: PreparedTokenLimitValues,
}

/// A target-bound receipt that clears one agent's active model generation.
pub struct PreparedAgentModelDeactivation {
    agent: tokio::sync::OwnedRwLockWriteGuard<ReactAgent>,
    critic_owner: Option<String>,
}

impl PreparedTokenLimit {
    /// Publish the token limit to the exact agent locked during preparation.
    pub fn commit(self) {
        let Self { mut agent, values } = self;
        agent.commit_prepared_token_limit(values);
    }
}

impl PreparedAgentModelGeneration {
    /// Publish this generation to the exact agent locked during preparation.
    /// No target parameter exists, so a receipt prepared on one agent cannot be
    /// committed to another agent.
    pub fn commit(self) {
        let Self {
            mut agent,
            llm_config,
            llm_client,
            temperature,
            max_tokens,
            thinking,
            critic,
            token_limit,
        } = self;
        agent.commit_prepared_token_limit(token_limit);
        agent.config.model_name = llm_config.model.clone();
        agent.config.temperature = temperature;
        agent.config.max_tokens = max_tokens;
        agent.llm_config = Some(llm_config);
        agent.llm_client = Some(llm_client);
        agent.thinking = thinking;
        match critic {
            PreparedCriticUpdate::ReplaceOwned { owner, critic }
                if agent.critic_owner.as_deref() == Some(owner.as_str()) =>
            {
                agent.critic = Some(critic);
            }
            PreparedCriticUpdate::Preserve | PreparedCriticUpdate::ReplaceOwned { .. } => {}
        }
    }
}

impl PreparedAgentModelDeactivation {
    /// Clear the model generation from the exact agent locked during preparation.
    pub fn commit(self) {
        let Self {
            mut agent,
            critic_owner,
        } = self;
        agent.config.model_name.clear();
        agent.config.temperature = None;
        agent.config.max_tokens = None;
        agent.llm_config = None;
        agent.llm_client = None;
        agent.thinking = None;
        if let Some(owner) = critic_owner
            && agent.critic_owner.as_deref() == Some(owner.as_str())
        {
            agent.critic = None;
            agent.critic_owner = None;
        }
    }
}

impl AgentHandle {
    /// Prepare a target-bound token-limit publication.
    pub async fn prepare_token_limit(
        &self,
        token_limit: usize,
    ) -> crate::error::Result<PreparedTokenLimit> {
        let agent = Arc::clone(self.inner()).write_owned().await;
        let values = agent.prepare_token_limit_values(token_limit).await?;
        Ok(PreparedTokenLimit { agent, values })
    }

    /// Prepare a target-bound complete model generation.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_model_generation(
        &self,
        llm_config: LlmConfig,
        llm_client: Arc<dyn LlmClient>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        thinking: Option<ThinkingConfig>,
        token_limit: usize,
        critic: PreparedCriticUpdate,
    ) -> crate::error::Result<PreparedAgentModelGeneration> {
        if llm_client.model_name() != llm_config.model {
            return Err(crate::error::ReactError::Other(format!(
                "prepared LLM client model '{}' does not match config model '{}'",
                llm_client.model_name(),
                llm_config.model
            )));
        }
        let agent = Arc::clone(self.inner()).write_owned().await;
        let token_limit = agent.prepare_token_limit_values(token_limit).await?;
        Ok(PreparedAgentModelGeneration {
            agent,
            llm_config,
            llm_client,
            temperature,
            max_tokens,
            thinking,
            critic,
            token_limit,
        })
    }

    /// Lock this agent for an atomic active-model deactivation.
    pub async fn prepare_model_deactivation(
        &self,
        critic_owner: Option<String>,
    ) -> PreparedAgentModelDeactivation {
        let agent = Arc::clone(self.inner()).write_owned().await;
        PreparedAgentModelDeactivation {
            agent,
            critic_owner,
        }
    }
}

impl ReactAgent {
    // ── Tool registration ────────────────────────────────────────────────────

    /// Register a single tool. Automatically enables tool capability.
    pub fn add_tool(&mut self, tool: Box<dyn Tool>) {
        self.config.enable_tool = true;
        self.tools.tool_manager.register(tool);
    }

    /// Register multiple tools. Automatically enables tool capability.
    pub fn add_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        if tools.is_empty() {
            return;
        }
        self.config.enable_tool = true;
        let allowed = &self.config.allowed_tools;
        if allowed.is_empty() {
            self.tools.tool_manager.register_tools(tools);
        } else {
            for tool in tools {
                if allowed.contains(&tool.name().to_string()) {
                    self.tools.tool_manager.register(tool);
                }
            }
        }
    }

    /// Remove a tool by name. Returns the removed tool if it existed.
    ///
    /// Supports dynamic tool management in the ReAct loop — for example,
    /// switching from search tools to execution tools mid-task.
    pub fn remove_tool(&mut self, name: &str) -> Option<Box<dyn Tool>> {
        self.tools.tool_manager.unregister(name)
    }

    /// Replace an existing tool with a new one of the same name.
    ///
    /// If a tool with the same name exists, it is removed and the new tool is registered.
    /// Returns the old tool if it was replaced.
    pub fn replace_tool(&mut self, tool: Box<dyn Tool>) -> Option<Box<dyn Tool>> {
        self.tools.tool_manager.replace(tool)
    }

    /// Register a tool that requires human approval before execution.
    ///
    /// This API is intentionally synchronous so it can be used during agent setup.
    /// When a `PermissionService` is present, the approval rule is first buffered in
    /// `pending_permission_rules` and transferred into the next invocation snapshot.
    /// This avoids calling `block_on()` inside a running Tokio runtime.
    ///
    /// Registers a permission rule requiring approval for the tool.
    pub fn add_need_appeal_tool(&mut self, tool: Box<dyn Tool>) {
        #[cfg(feature = "human-loop")]
        if self.config.enable_human_in_loop {
            let tool_name = tool.name().to_string();
            self.add_tool(tool);

            use echo_core::tools::permission::{PermissionRule, RuleMatcher, RuleSource};
            let rule = PermissionRule {
                matcher: RuleMatcher::Pattern {
                    pattern: tool_name.clone(),
                },
                behavior: echo_core::tools::permission::RuleBehavior::Ask {
                    suggestions: vec!["Allow".to_string(), "Deny".to_string()],
                },
                source: RuleSource::Session,
                description: Some(format!("Tool {} requires human approval", tool_name)),
            };

            // add_need_appeal_tool is a synchronous API and cannot call block_on()
            // inside a running Tokio runtime. Register the rule here first, then
            // safely flush it to PermissionService during a later async execution phase.
            if let Ok(mut pending) = self.approval.pending_permission_rules.lock() {
                pending.push(rule);
            } else {
                warn!(
                    agent = %self.config.agent_name,
                    tool = %tool_name,
                    "pending_permission_rules lock poisoned"
                );
            }

            return;
        }
        #[cfg(not(feature = "human-loop"))]
        let _ = &self.config.enable_human_in_loop;
        warn!(
            agent = %self.config.agent_name,
            tool = %tool.name(),
            "human_in_loop disabled, tool registered without approval requirement"
        );
        self.add_tool(tool);
    }

    // ── Context compression ──────────────────────────────────────────────────

    /// Set context compressor
    ///
    /// # Parameters
    /// * `compressor` - Compressor instance implementing the `ContextCompressor` trait
    ///
    /// # Description
    /// The context compressor automatically compresses conversation history when the token count exceeds the limit,
    /// removing less important messages to reduce token consumption.
    pub async fn set_compressor(&self, compressor: impl ContextCompressor + 'static) {
        self.memory.context.lock().await.set_compressor(compressor);
    }

    /// Get context statistics
    ///
    /// # Returns
    /// Returns tuple `(message count, estimated token count)`
    pub async fn context_stats(&self) -> (usize, usize) {
        let ctx = self.memory.context.lock().await;
        (ctx.messages().len(), ctx.token_estimate())
    }

    /// Force compress context using the specified compressor
    ///
    /// # Parameters
    /// * `compressor` - Compressor instance to use for compression
    ///
    /// # Returns
    /// Returns compression statistics, including token counts before and after
    ///
    /// # Description
    /// This method bypasses the automatic compression threshold and immediately compresses the context
    /// using the specified compressor. Typically used for manual compression control or testing compression effects.
    pub async fn force_compress_with(
        &self,
        compressor: &dyn ContextCompressor,
    ) -> Result<(ForceCompressStats, Option<CompressionCheckpoint>)> {
        self.memory
            .context
            .lock()
            .await
            .force_compress_with(compressor)
            .await
    }

    /// Force compress with PreCompact/PostCompact hooks fired around the operation.
    ///
    /// The `matcher` parameter determines the hook matcher context:
    /// - `"manual"` for user-initiated compression (e.g. `/compact` command)
    /// - `"auto"` for automatic threshold-triggered compression
    pub async fn force_compress_with_hooks(
        &self,
        compressor: &dyn ContextCompressor,
        matcher: &str,
    ) -> Result<(ForceCompressStats, Option<CompressionCheckpoint>)> {
        self.fire_lifecycle_hook(crate::skills::hooks::HookEvent::PreCompact, Some(matcher))
            .await;
        let (stats, checkpoint) = self.force_compress_with(compressor).await?;
        self.fire_post_compact_hook(matcher, &stats).await;
        Ok((stats, checkpoint))
    }

    /// Force-compress the context with focus instructions and PreCompact/PostCompact hooks.
    pub async fn force_compress_with_focus_and_hooks(
        &self,
        focus_instructions: &str,
        window: usize,
        matcher: &str,
    ) -> Result<(ForceCompressStats, Option<CompressionCheckpoint>)> {
        self.fire_lifecycle_hook(crate::skills::hooks::HookEvent::PreCompact, Some(matcher))
            .await;
        let (stats, checkpoint) = self
            .memory
            .context
            .lock()
            .await
            .force_compress_with_focus(focus_instructions, window)
            .await?;
        self.fire_post_compact_hook(matcher, &stats).await;
        Ok((stats, checkpoint))
    }

    async fn fire_post_compact_hook(&self, matcher: &str, stats: &ForceCompressStats) {
        let hook_stats = crate::skills::hooks::CompressHookStats {
            before_count: stats.before_count,
            after_count: stats.after_count,
            before_tokens: stats.before_tokens,
            after_tokens: stats.after_tokens,
        };
        let hook_ctx = crate::skills::hooks::HookContext::for_post_compact(
            &hook_stats,
            matcher,
            self.config.session_id.as_deref().unwrap_or(""),
            &self.config.agent_name,
        );
        let registry = self.tools.hook_registry.read().await.clone();
        let post_result = registry.run_lifecycle_hooks(&hook_ctx).await;
        if let Some(ctx) = &post_result.injected_context {
            super::run::context::push_runtime_context_note(
                &self.memory.context,
                "Hook:PostCompact",
                ctx,
            )
            .await;
        }
        for msg in &post_result.messages {
            super::run::context::push_runtime_context_note(
                &self.memory.context,
                "Hook:PostCompact",
                msg,
            )
            .await;
        }
    }

    /// Force-compress the context using the installed compressor (or fallback
    /// SlidingWindowCompressor with window=40 if no compressor is installed).
    ///
    /// Designed for manual compression triggers (desktop UI button, `/compact` CLI command).
    /// Fires PreCompact/PostCompact hooks with matcher `"manual"`.
    pub async fn force_compress_context(
        &self,
    ) -> Result<(ForceCompressStats, Option<CompressionCheckpoint>)> {
        self.fire_lifecycle_hook(crate::skills::hooks::HookEvent::PreCompact, Some("manual"))
            .await;
        let (stats, checkpoint) = self.memory.context.lock().await.force_compress(40).await?;
        let (protected_message_count, protected_context_tokens) = {
            let context = self.memory.context.lock().await;
            (
                context.protected_message_count(),
                context.protected_token_estimate(),
            )
        };
        self.record_trace_event(crate::trace::RunEvent::ContextCompression {
            source: "manual".to_string(),
            before_messages: stats.before_count,
            after_messages: stats.after_count,
            before_tokens: stats.before_tokens,
            after_tokens: stats.after_tokens,
            protected_context_tokens,
            protected_message_count,
        })
        .await;
        self.fire_post_compact_hook("manual", &stats).await;
        Ok((stats, checkpoint))
    }

    /// List all registered tool names
    ///
    /// # Returns
    /// List of registered tool names
    pub fn list_tools(&self) -> Vec<String> {
        self.tools.tool_manager.list_tools()
    }

    // ── Subagent ─────────────────────────────────────────────────────────────

    /// Register a subagent with the subagent registry.
    ///
    /// The agent is automatically wrapped in a default Sync-mode `SubagentDefinition`.
    /// For more control, use `register_subagent_with_definition()`.
    #[cfg(feature = "subagent")]
    pub fn register_agent(&mut self, agent: Box<dyn Agent>) {
        let name = agent.name().to_string();
        let def = SubagentDefinition::simple_sync(&name);
        self.register_subagent_with_definition(def, agent);
    }

    /// Register a subagent with an explicit definition.
    ///
    /// The definition is exposed to the orchestrator through `agent_tool`, so
    /// descriptions should be concise and task-oriented.
    #[cfg(feature = "subagent")]
    pub fn register_subagent_with_definition(
        &mut self,
        def: SubagentDefinition,
        agent: Box<dyn Agent>,
    ) {
        // Note: subagent registration is decoupled from `enable_subagent` flag.
        // `enable_subagent` historically controlled two things: (1) subagent
        // registration here, and (2) `AgentDispatchTool` LLM tool registration
        // in `ReactAgent::new`. They are now split: subagent registration is
        // unconditional (framework dispatch via `delegate_to_agent*` depends on
        // it), and LLM tool registration is gated by `register_agent_dispatch_tool`.
        let name = def.name.clone();
        if self
            .tools
            .subagent_registry
            .register_sync(def.clone(), agent)
        {
            info!(agent = %self.config.agent_name, subagent = %name, "Subagent registered");
        }
    }

    /// Register a subagent **definition only** — no instance and no factory.
    ///
    /// This is a low-level discovery / late-binding entry point for runtimes
    /// that can guarantee later hydration: a definition becomes visible to
    /// `list_available`, `agent_names`, and the dispatch catalog before any
    /// executable instance exists. The application layer (which owns the
    /// prompt-compiler / tool-filter / sandbox wiring needed to build a real
    /// `ReactAgent` instance) later supplies the instance via
    /// [`register_subagent_with_definition`](Self::register_subagent_with_definition)
    /// or [`register_subagent_factory`](Self::register_subagent_factory)
    /// under the same name, overwriting the definition.
    ///
    /// Dispatching a definition registered this way **without** subsequently
    /// providing an instance will fail at execution time (no agent to run).
    #[cfg(feature = "subagent")]
    pub fn register_subagent_definition(&mut self, def: SubagentDefinition) {
        let name = def.name.clone();
        let inserted = self
            .tools
            .subagent_registry
            .register_definition_sync(def.clone());
        if inserted {
            info!(
                agent = %self.config.agent_name,
                subagent = %name,
                "Subagent definition registered (late-binding, no instance)"
            );
        }
    }

    /// Register a factory used to create an independent agent for each isolated dispatch.
    #[cfg(feature = "subagent")]
    pub fn register_subagent_factory(
        &mut self,
        def: SubagentDefinition,
        factory: Arc<dyn AgentFactory>,
    ) {
        let name = def.name.clone();
        if self
            .tools
            .subagent_registry
            .register_factory_sync(def.clone(), factory)
        {
            info!(agent = %self.config.agent_name, subagent = %name, "Subagent factory registered");
        }
    }

    /// Unregister a Subagent and remove it from the model-facing dispatch
    /// catalog. Returns whether a definition existed before removal.
    #[cfg(feature = "subagent")]
    pub async fn unregister_subagent(&mut self, name: &str) -> bool {
        let existed = self.tools.subagent_registry.contains(name).await;
        self.tools.subagent_registry.remove(name).await;
        existed
    }

    /// Refresh the cached tool schema for legacy callers. The dispatch catalog
    /// itself is read from the shared registry and ignores the supplied copy.
    #[cfg(feature = "subagent")]
    pub fn sync_subagent_dispatch_catalog(&self, _definitions: &[SubagentDefinition]) {
        self.tools.tool_manager.invalidate_definition_cache();
    }

    /// Batch register subagents
    ///
    /// # Parameters
    /// * `agents` - List of subagent instances
    ///
    /// # Description
    /// Each subagent is automatically wrapped in a default Sync-mode `SubagentDefinition`.
    /// For more fine-grained control, use the `register_subagent_with_definition()` method.
    #[cfg(feature = "subagent")]
    pub fn register_agents(&mut self, agents: Vec<Box<dyn Agent>>) {
        for agent in agents {
            self.register_agent(agent)
        }
    }

    // ── Basic config ─────────────────────────────────────────────────────────

    /// Set LLM model name at runtime
    ///
    /// # Parameters
    /// * `model_name` - New LLM model name
    ///
    /// # Description
    /// This method allows dynamically switching the LLM model used by the Agent at runtime.
    pub fn set_model(&mut self, model_name: &str) {
        self.config.model_name = model_name.to_string();
    }

    /// Set the temperature for LLM generation.
    ///
    /// # Parameters
    /// * `temperature` - Temperature value (typically 0.0 - 2.0)
    pub fn set_temperature(&mut self, temperature: Option<f32>) {
        self.config.temperature = temperature;
    }

    /// Set the maximum number of tokens for LLM generation.
    ///
    /// # Parameters
    /// * `max_tokens` - Maximum tokens to generate
    pub fn set_max_tokens(&mut self, max_tokens: Option<u32>) {
        self.config.max_tokens = max_tokens;
    }

    /// Prepare a token-limit change without mutating the agent.
    ///
    /// This waits for exclusive access to the live context and retains that
    /// access in the returned receipt. Dropping the receipt rolls the operation
    /// back; target-bound publication commit is infallible.
    async fn prepare_token_limit_values(
        &self,
        token_limit: usize,
    ) -> crate::error::Result<PreparedTokenLimitValues> {
        let budget = if self.config.token_budget_config.enabled {
            Some(self.config.token_budget_config.build(token_limit)?)
        } else {
            None
        };
        let context = Arc::clone(&self.memory.context).lock_owned().await;
        Ok(PreparedTokenLimitValues {
            token_limit,
            budget,
            context,
        })
    }

    fn commit_prepared_token_limit(&mut self, prepared: PreparedTokenLimitValues) {
        let PreparedTokenLimitValues {
            token_limit,
            budget,
            mut context,
        } = prepared;
        context.set_token_limit(token_limit, budget);
        self.config.token_limit = token_limit;
    }

    /// Set the token limit (context window budget) at runtime.
    ///
    /// This compatibility method remains non-blocking. Consumers that need a
    /// transactional multi-agent update should use the prepare/commit API.
    pub fn set_token_limit(&mut self, token_limit: usize) -> crate::error::Result<()> {
        let mut context = self.memory.context.try_lock().map_err(|_| {
            crate::error::AgentError::ContextLimitExceeded(
                "cannot change the context window while an agent turn is active".to_string(),
            )
        })?;
        let budget = if self.config.token_budget_config.enabled {
            Some(self.config.token_budget_config.build(token_limit)?)
        } else {
            None
        };
        context.set_token_limit(token_limit, budget);
        self.config.token_limit = token_limit;
        Ok(())
    }

    /// Set agent-level default tools hidden from the LLM on subsequent runs.
    ///
    /// Tools whose names appear in `names` are filtered out of the tool list
    /// sent to the model, so the model cannot call them. The tools remain
    /// registered (unlike `remove_tool`, which mutates the shared registry).
    /// Pass `None` or an empty set to clear.
    ///
    /// The value is captured when an `AgentRunSnapshot` is created. Existing
    /// runs are unaffected. Per-invocation exclusions should be supplied via
    /// `AgentInvocationContext::disabled_tools`.
    pub fn set_disabled_tools(&self, names: Option<std::collections::HashSet<String>>) {
        self.tools.set_disabled_tools(names);
    }

    /// Add Agent callback
    ///
    /// # Parameters
    /// * `callback` - Callback instance implementing the `AgentCallback` trait
    ///
    /// # Description
    /// Callbacks are invoked when different events are triggered during Agent execution, for monitoring, logging, etc.
    /// Unlike the config builder's `with_callback`, this method can add callbacks dynamically at runtime.
    pub fn add_callback(&mut self, callback: Arc<dyn crate::agent::AgentCallback>) {
        self.config.callbacks.push(callback);
    }

    /// Remove all callbacks whose type name matches the given string.
    ///
    /// Useful for cleaning up temporary callbacks (e.g., progress bridges)
    /// after a task completes.
    pub fn remove_callbacks_by_type_name(&mut self, type_name: &str) {
        self.config.callbacks.retain(|cb| {
            let name = std::any::type_name_of_val(cb.as_ref()).to_string();
            let kind_matches = cb
                .callback_kind()
                .map(|kind| kind.contains(type_name))
                .unwrap_or_else(|| name.contains(type_name));
            !kind_matches
        });
    }

    /// Remove callbacks of a given type whose stable callback ID matches.
    ///
    /// Useful for task-scoped temporary callbacks such as `ProgressBridge`,
    /// where removing by type alone can delete another active task's callback.
    pub fn remove_callbacks_by_type_name_and_id(&mut self, type_name: &str, callback_id: &str) {
        self.config.callbacks.retain(|cb| {
            let name = std::any::type_name_of_val(cb.as_ref()).to_string();
            let kind_matches = cb
                .callback_kind()
                .map(|kind| kind.contains(type_name))
                .unwrap_or_else(|| name.contains(type_name));
            !(kind_matches && cb.callback_id() == Some(callback_id))
        });
    }

    /// Access the execution serializer.
    ///
    /// Use this when you need to hold the execution lock across multiple
    /// agent operations (e.g., registering a temporary callback then
    /// executing, where both steps must be atomic with respect to other
    /// concurrent executions).
    ///
    /// For normal `execute()` / `chat()` / `chat_stream()` calls, the
    /// lock is acquired automatically inside `run_react_loop` / `run_loop`.
    pub fn execution_mutex(&self) -> &Arc<tokio::sync::Mutex<()>> {
        &self.execution_mutex
    }

    // ── Skills (code-based) ──────────────────────────────────────────────────

    /// Install a code-based skill (eager: tools + prompt injected immediately).
    pub fn add_skill(&mut self, skill: Box<dyn Skill>) {
        let name = skill.name().to_string();

        if self.tools.skill_registry.is_installed(&name) {
            warn!(
                agent = %self.config.agent_name,
                skill = %name,
                "Skill already installed, skipping"
            );
            return;
        }

        let sandbox = self
            .tools
            .sandbox_manager
            .as_ref()
            .map(|manager| manager.clone() as Arc<dyn crate::sandbox::SandboxExecutor>);
        let tools = skill.tools_with_sandbox(sandbox);
        let tool_names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();

        for tool in tools {
            self.tools.tool_manager.register(tool);
        }

        let has_injection = skill.system_prompt_injection().is_some();
        if let Some(injection) = skill.system_prompt_injection() {
            self.config.system_prompt.push_str(&injection);
            self.memory
                .context
                .try_lock()
                .map(|mut ctx| ctx.update_system(self.config.system_prompt.clone()))
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to acquire context lock for skill injection: {}", e);
                });
        }

        self.tools.skill_registry.record_code_skill(SkillInfo {
            name: name.clone(),
            description: skill.description().to_string(),
            tool_names,
            has_prompt_injection: has_injection,
        });

        info!(
            agent = %self.config.agent_name,
            skill = %name,
            description = %skill.description(),
            "Skill installed"
        );
    }

    /// Install multiple code-based skills.
    pub fn add_skills(&mut self, skills: Vec<Box<dyn Skill>>) {
        for skill in skills {
            self.add_skill(skill);
        }
    }

    // ── Skills (file-based, progressive disclosure) ──────────────────────────

    /// Discover file-based skills from the given scopes.
    ///
    /// Implements the agentskills.io progressive disclosure model:
    /// 1. Parse `SKILL.md` frontmatter (name + description) from each scope
    /// 2. Build a compact **catalog** and inject it into the system prompt
    /// 3. Register `activate_skill` and `read_skill_resource` tools
    ///
    /// The LLM can then decide which skills to activate on demand.
    pub async fn discover_skills(&mut self, scopes: &[DiscoveryScope]) -> Result<Vec<String>> {
        self.discover_skills_inner(scopes, None).await
    }

    async fn discover_skills_inner(
        &mut self,
        scopes: &[DiscoveryScope],
        plugin: Option<(&str, &crate::plugin::PluginVariables)>,
    ) -> Result<Vec<String>> {
        let mut loader = match &self.skill_load_policy {
            Some(policy) => SkillLoader::new().with_policy(policy.clone()),
            None => SkillLoader::new(),
        };
        if let Some((_, variables)) = plugin {
            loader = loader.with_plugin_variables(variables.clone());
        }
        let descriptors = match plugin {
            Some(_) => {
                let directory = scopes.iter().find_map(|scope| match scope {
                    DiscoveryScope::Custom(path) => Some(path.clone()),
                    DiscoveryScope::Project(_) | DiscoveryScope::User => None,
                });
                match directory {
                    Some(directory) => loader.discover_agent_plugin_skills(directory).await?,
                    None => Vec::new(),
                }
            }
            None => loader.discover(scopes).await?,
        };

        if descriptors.is_empty() {
            info!(
                agent = %self.config.agent_name,
                "No skills found during discovery"
            );
            return Ok(vec![]);
        }

        let mut names = Vec::new();

        // Build a shared registry for the progressive disclosure tools.
        // This is separate from `self.tools.skill_registry.lock().unwrap()` (which tracks code-based skills).
        // The shared registry holds descriptors + activation state, accessed by
        // both ActivateSkillTool and ReadSkillResourceTool during async execution.
        let shared = if let Some(existing) = &self.tools.progressive_skill_registry {
            existing.clone()
        } else {
            let reg = Arc::new(RwLock::new(crate::skills::SkillRegistry::new()));
            if let Some(ref manager) = self.tools.sandbox_manager {
                if let Ok(mut guard) = reg.try_write() {
                    guard.set_sandbox_manager(manager.clone());
                }
                self.tools
                    .skill_registry
                    .set_sandbox_manager(manager.clone());
            }
            self.tools.progressive_skill_registry = Some(reg.clone());
            reg
        };

        {
            let mut reg = shared.write().await;
            if let Some(ref manager) = self.tools.sandbox_manager {
                reg.set_sandbox_manager(manager.clone());
                self.tools
                    .skill_registry
                    .set_sandbox_manager(manager.clone());
            }
            for mut desc in descriptors {
                if self.tools.skill_registry.is_installed(&desc.name) {
                    warn!(
                        agent = %self.config.agent_name,
                        skill = %desc.name,
                        "Skill already installed, skipping duplicate"
                    );
                    continue;
                }

                let legacy = loader.get_legacy_instructions(&desc.name).cloned();
                if let Some((source, _)) = plugin {
                    desc.source = Some(source.to_string());
                }

                // Register hooks from frontmatter if present
                if let Some(hooks_def) = &desc.hooks {
                    let skill_dir = desc
                        .location
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    let mut hook_reg = self.tools.hook_registry.write().await;
                    hook_reg.register(&desc.name, &skill_dir, hooks_def.clone());
                }

                let name = desc.name.clone();
                names.push(name.clone());
                self.tools
                    .skill_registry
                    .register_descriptor_with_legacy(desc.clone(), legacy.clone());
                reg.register_descriptor_with_legacy(desc, legacy);
                if let Some((source, variables)) = plugin {
                    self.tools.skill_registry.tag_source_with_variables(
                        std::slice::from_ref(&name),
                        source,
                        Some(variables),
                    );
                    reg.tag_source_with_variables(
                        std::slice::from_ref(&name),
                        source,
                        Some(variables),
                    );
                }
            }
        }

        if names.is_empty() {
            return Ok(names);
        }

        // Keep the catalog independently replaceable. Mutating the root system
        // prompt on every discovery pass duplicates content and invalidates the
        // provider's longest cacheable prefix.
        if let Some(catalog) = self.tools.skill_registry.catalog_prompt() {
            self.memory.context.lock().await.replace_projection(
                SKILL_CATALOG_PROJECTION,
                Some(crate::llm::types::Message::system(catalog)),
            );
        }

        // Fire InstructionsLoaded hook
        {
            let hook_ctx = crate::skills::hooks::HookContext::for_instructions_loaded(
                "startup",
                &names,
                self.config.session_id.as_deref().unwrap_or(""),
                &self.config.agent_name,
            );
            let registry = self.tools.hook_registry.read().await.clone();
            let result = registry.run_lifecycle_hooks(&hook_ctx).await;
            if result.block {
                warn!(agent = %self.config.agent_name, reason = ?result.block_reason, "InstructionsLoaded hook blocked");
            }
            if let Some(ctx) = &result.injected_context {
                super::run::context::push_runtime_context_note(
                    &self.memory.context,
                    "Hook:InstructionsLoaded",
                    ctx,
                )
                .await;
            }
            for msg in &result.messages {
                super::run::context::push_runtime_context_note(
                    &self.memory.context,
                    "Hook:InstructionsLoaded",
                    msg,
                )
                .await;
            }
        }

        // Register progressive disclosure tools with shared registry.
        // We refresh these tool instances on every discovery pass so their internal
        // registry and available skill name list stay in sync with newly discovered
        // skills across repeated `discover_skills()` calls.
        let available_names = self.tools.skill_registry.available_names();

        self.replace_tool(Box::new(ActivateSkillTool::new(
            shared.clone(),
            available_names,
        )));
        self.replace_tool(Box::new(ReadSkillResourceTool::new(shared.clone())));

        let mut script_tool = RunSkillScriptTool::new(shared);
        if let Some(ref manager) = self.tools.sandbox_manager {
            script_tool = script_tool.with_sandbox_manager(manager.clone());
        }
        self.replace_tool(Box::new(script_tool));

        info!(
            agent = %self.config.agent_name,
            count = names.len(),
            skills = ?names,
            "Skills discovered and catalog injected"
        );

        Ok(names)
    }

    /// Remove already-discovered skills that are no longer allowed by the
    /// current product policy, then refresh catalog and progressive tools.
    pub async fn reconcile_skill_load_policy(&mut self) -> Vec<String> {
        let Some(policy) = &self.skill_load_policy else {
            return Vec::new();
        };
        let removed: Vec<String> = self
            .tools
            .skill_registry
            .list_descriptors()
            .into_iter()
            .filter(|descriptor| !policy.allows(descriptor))
            .map(|descriptor| descriptor.name.clone())
            .collect();
        if removed.is_empty() {
            return removed;
        }

        for name in &removed {
            self.tools.skill_registry.remove_descriptor(name);
            self.tools
                .hook_registry
                .write()
                .await
                .unregister(&crate::skills::hooks::HookSource::Skill(name.clone()));
            self.memory
                .context
                .lock()
                .await
                .replace_projection(format!("echo-agent:skill:{name}"), None);
        }

        if let Some(shared) = &self.tools.progressive_skill_registry {
            let mut registry = shared.write().await;
            for name in &removed {
                registry.remove_descriptor(name);
            }
        }

        let catalog = self
            .tools
            .skill_registry
            .catalog_prompt()
            .map(crate::llm::types::Message::system);
        self.memory
            .context
            .lock()
            .await
            .replace_projection(SKILL_CATALOG_PROJECTION, catalog);

        if let Some(shared) = self.tools.progressive_skill_registry.clone() {
            let available_names = self.tools.skill_registry.available_names();
            self.replace_tool(Box::new(ActivateSkillTool::new(
                shared.clone(),
                available_names,
            )));
            self.replace_tool(Box::new(ReadSkillResourceTool::new(shared.clone())));
            let mut script_tool = RunSkillScriptTool::new(shared);
            if let Some(manager) = &self.tools.sandbox_manager {
                script_tool = script_tool.with_sandbox_manager(manager.clone());
            }
            self.replace_tool(Box::new(script_tool));
        }

        info!(
            agent = %self.config.agent_name,
            skills = ?removed,
            "Skills removed by load policy reconciliation"
        );
        removed
    }

    /// Discover skills from one explicit directory.
    pub async fn load_skills_from_dir(
        &mut self,
        skills_dir: impl Into<std::path::PathBuf>,
    ) -> Result<Vec<String>> {
        self.discover_skills(&[DiscoveryScope::Custom(skills_dir.into())])
            .await
    }

    /// Discover plugin-owned skills with variable substitution applied before
    /// frontmatter hooks are parsed and registered.
    pub async fn load_plugin_skills_from_dir(
        &mut self,
        skills_dir: impl Into<std::path::PathBuf>,
        source: &str,
        variables: &crate::plugin::PluginVariables,
    ) -> Result<Vec<String>> {
        self.discover_skills_inner(
            &[DiscoveryScope::Custom(skills_dir.into())],
            Some((source, variables)),
        )
        .await
    }

    /// Mark newly discovered skills with one component source in both live
    /// registries. The progressive registry powers activation/resource tools;
    /// tagging only the catalog registry would leave disabled plugin skills
    /// executable through those tools.
    pub async fn tag_skills_source(&mut self, names: &[String], source: &str) {
        self.tools.skill_registry.tag_source(names, source);
        if let Some(shared) = &self.tools.progressive_skill_registry {
            shared.write().await.tag_source(names, source);
        }
    }

    /// Remove every skill owned by `source` and refresh all projections/tools.
    pub async fn unregister_skills_by_source(&mut self, source: &str) -> Vec<String> {
        let removed = self.tools.skill_registry.unregister_names_by_source(source);
        if removed.is_empty() {
            return removed;
        }

        if let Some(shared) = &self.tools.progressive_skill_registry {
            shared.write().await.unregister_by_source(source);
        }
        {
            let mut hooks = self.tools.hook_registry.write().await;
            for name in &removed {
                hooks.unregister(&crate::skills::hooks::HookSource::Skill(name.clone()));
            }
        }
        {
            let mut context = self.memory.context.lock().await;
            for name in &removed {
                context.replace_projection(format!("echo-agent:skill:{name}"), None);
            }
            let catalog = self
                .tools
                .skill_registry
                .catalog_prompt()
                .map(crate::llm::types::Message::system);
            context.replace_projection(SKILL_CATALOG_PROJECTION, catalog);
        }

        if let Some(shared) = self.tools.progressive_skill_registry.clone() {
            let available_names = self.tools.skill_registry.available_names();
            self.replace_tool(Box::new(ActivateSkillTool::new(
                shared.clone(),
                available_names,
            )));
            self.replace_tool(Box::new(ReadSkillResourceTool::new(shared.clone())));
            let mut script_tool = RunSkillScriptTool::new(shared);
            if let Some(manager) = &self.tools.sandbox_manager {
                script_tool = script_tool.with_sandbox_manager(manager.clone());
            }
            self.replace_tool(Box::new(script_tool));
        }

        removed
    }

    /// Activate one installed skill and inject its instructions into context.
    ///
    /// All activation paths should use this method so registry state and model
    /// context cannot diverge.
    pub async fn activate_skill(&self, skill_name: &str) -> Result<()> {
        if !self.tools.skill_registry.is_installed(skill_name) {
            return Ok(());
        }

        let projection_marker = format!("echo-agent:skill:{skill_name}");
        if self.tools.skill_registry.is_activated(skill_name)
            && self
                .memory
                .context
                .lock()
                .await
                .has_projection(projection_marker.as_str())
        {
            return Ok(());
        }

        let content = self.tools.skill_registry.activate(skill_name).await?;
        let block = content.to_prompt_block();
        project_skill_activation(&self.memory.context, skill_name, &block).await;

        tracing::info!(
            agent = %self.config.agent_name,
            skill = %skill_name,
            "Skill activated and injected as a replaceable context projection"
        );
        Ok(())
    }

    /// Replace or remove a named system-context projection.
    ///
    /// Product runtimes use this for user-selectable instruction artifacts
    /// such as output styles. The marker is the stable ownership boundary, so
    /// replacing or unloading one projection never mutates the base prompt or
    /// unrelated runtime context.
    pub async fn replace_system_context_projection(
        &self,
        marker: impl Into<String>,
        content: Option<String>,
    ) {
        let message = content.map(crate::llm::types::Message::system);
        self.memory
            .context
            .lock()
            .await
            .replace_projection(marker.into(), message);
    }

    /// List all installed code-based skills.
    pub fn list_skills(&self) -> Vec<SkillInfo> {
        self.tools
            .skill_registry
            .list()
            .into_iter()
            .cloned()
            .collect()
    }

    /// Check if a skill (code or file-based) is installed.
    pub fn has_skill(&self, name: &str) -> bool {
        self.tools.skill_registry.is_installed(name)
    }

    /// Total number of installed skills (code + file-based).
    pub fn skill_count(&self) -> usize {
        self.tools.skill_registry.count()
    }

    /// Get the shared skill registry handle (for external tool access).
    pub fn skill_registry(&self) -> &crate::skills::SkillRegistry {
        &self.tools.skill_registry
    }

    /// Get a mutable reference to the skill registry (for registering descriptors).
    pub fn skill_registry_mut(&mut self) -> &mut crate::skills::SkillRegistry {
        &mut self.tools.skill_registry
    }

    /// Get the shared hook registry handle.
    pub fn hook_registry(&self) -> &Arc<tokio::sync::RwLock<crate::skills::hooks::HookRegistry>> {
        &self.tools.hook_registry
    }

    /// Get the shared tool manager handle.
    pub fn tool_manager(&self) -> &Arc<echo_execution::tools::ToolManager> {
        &self.tools.tool_manager
    }

    /// Get the sandbox manager, if configured.
    pub fn sandbox_manager(&self) -> Option<&Arc<crate::sandbox::SandboxManager>> {
        self.tools.sandbox_manager.as_ref()
    }

    /// Get the tool execution pipeline, if configured.
    pub fn tool_execution_pipeline(
        &self,
    ) -> &Option<Arc<crate::agent::react::run::pipeline::ToolExecutionPipeline>> {
        &self.tool_execution_pipeline
    }

    /// Get the runtime state store, if configured.
    pub fn state_store(&self) -> &Option<Arc<dyn crate::state::RuntimeStateStore>> {
        &self.memory.state_store
    }

    /// Get the conversation store, if configured.
    pub fn conversation_store(&self) -> &Option<Arc<dyn crate::memory::ConversationStore>> {
        &self.memory.conversation_store
    }

    #[cfg(feature = "human-loop")]
    /// Get the permission service, if configured.
    pub fn permission_service(
        &self,
    ) -> Option<&Arc<crate::human_loop::service::PermissionService>> {
        self.approval.permission_service.as_ref()
    }

    /// Create a `TaskHookBridge` that lets an application-owned task runtime
    /// translate its durable lifecycle events into the central registry.
    ///
    /// Task runtime adapters invoke lifecycle hooks at `TaskRevisionService`
    /// commit and `RuntimeDagController` execution boundaries.
    ///
    /// The bridge fires task lifecycle events
    /// (`TaskCreated`, `TaskStarted`, and terminal `TaskCompleted`) into the
    /// central `HookRegistry`. Terminal reason is carried by status.
    pub fn create_task_hook_bridge(&self) -> crate::hooks_bridge::TaskHookBridge {
        crate::hooks_bridge::TaskHookBridge::new(
            self.tools.hook_registry.clone(),
            self.config
                .session_id
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            self.config.agent_name.clone(),
        )
    }

    /// Create a `SubagentHookBridge` that fires subagent lifecycle events
    /// (SubagentStart, SubagentStop) into the central `HookRegistry`.
    /// SubagentStop carries a terminal status (completed/failed/cancelled/
    /// timed_out); there is no separate SubagentCancelled event.
    pub fn create_subagent_hook_bridge(&self) -> crate::hooks_bridge::SubagentHookBridge {
        crate::hooks_bridge::SubagentHookBridge::new(
            self.tools.hook_registry.clone(),
            self.config
                .session_id
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            self.config.agent_name.clone(),
        )
    }

    // ── MCP ──────────────────────────────────────────────────────────────────

    /// Register MCP (Model Context Protocol) tools
    ///
    /// # Parameters
    /// * `tools` - List of MCP tool instances
    ///
    /// # Description
    /// Registers MCP tools into the Agent's tool manager, making them available for tool calls.
    pub fn register_mcp_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        self.add_tools(tools);
    }

    #[cfg(feature = "mcp")]
    fn sync_mcp_resource_tools(&mut self) {
        for name in crate::mcp::MCP_RESOURCE_TOOL_NAMES {
            self.remove_tool(name);
        }
        let tools = self.tools.mcp_manager.resource_tools();
        self.add_tools(tools);
    }

    #[cfg(feature = "mcp")]
    /// Connect to an MCP server based on MCP server configuration
    ///
    /// # Parameters
    /// * `config` - MCP server configuration
    ///
    /// # Returns
    /// Returns the `McpClient` instance created after successful connection
    ///
    /// # Description
    /// This method connects to the MCP server based on the configuration, retrieves the tools provided by the server,
    /// and registers them into the Agent's tool manager.
    pub async fn connect_mcp_from_config(
        &mut self,
        config: McpServerConfig,
    ) -> crate::error::Result<Arc<McpClient>> {
        let name = config.name.clone();
        // Reconnection must also remove tools that disappeared from the new
        // server schema. McpManager replaces the client, but it cannot mutate
        // the agent's ToolManager on its own.
        if self.tools.mcp_manager.get_client(&name).is_some() {
            self.disconnect_mcp(&name).await;
        }
        let tools = self.tools.mcp_manager.connect(config).await?;
        let count = tools.len();
        self.add_tools(tools);
        self.sync_mcp_resource_tools();
        let client = {
            let mgr = &self.tools.mcp_manager;
            mgr.get_client(&name).ok_or_else(|| {
                crate::error::ReactError::Agent(Box::new(
                    crate::error::AgentError::InitializationFailed(format!(
                        "MCP client '{}' not found after connection",
                        name
                    )),
                ))
            })?
        };
        tracing::info!(
            agent = %self.config.agent_name,
            server = %name,
            tools = count,
            "MCP server connected"
        );
        self.setup_hook_mcp_executor().await;
        Ok(client)
    }

    #[cfg(feature = "mcp")]
    /// Connect to an MCP server from a JSON string configuration
    ///
    /// # Parameters
    /// * `name` - MCP server name
    /// * `json_config_str` - JSON-formatted MCP server configuration string
    ///
    /// # Returns
    /// Returns the `McpClient` instance created after successful connection
    ///
    /// # Description
    /// This method parses the JSON-formatted configuration string, converts it to MCP server configuration,
    /// then calls `connect_mcp_from_config` to connect.
    pub async fn connect_mcp_from_json(
        &mut self,
        name: &str,
        json_config_str: &str,
    ) -> crate::error::Result<Arc<McpClient>> {
        let entry = serde_json::from_str::<McpServerEntry>(json_config_str)?;
        let config = entry.to_server_config(name)?;
        self.connect_mcp_from_config(config).await
    }

    #[cfg(feature = "mcp")]
    /// Load and connect to multiple MCP servers from a configuration file
    ///
    /// # Parameters
    /// * `path` - MCP configuration file path (supports `.json` or `.yaml` format)
    ///
    /// # Returns
    /// Returns a list of successfully connected `McpClient` instances
    ///
    /// # Description
    /// 1. Parse the MCP configuration file (supports JSON or YAML format)
    /// 2. Call `connect_mcp_from_config` for each server configuration
    /// 3. Collect all successfully connected clients
    /// 4. Failed server connections are skipped with a warning log
    ///
    /// # Example
    /// ```rust
    /// use echo_agent::prelude::*;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> echo_agent::error::Result<()> {
    /// let path = std::env::temp_dir().join(format!(
    ///     "echo-agent-doctest-mcp-{}.json",
    ///     std::process::id()
    /// ));
    /// std::fs::write(&path, r#"{"mcpServers":{}}"#)?;
    ///
    /// let config = AgentConfig::new("qwen3-max", "mcp_agent", "You are a helpful assistant");
    /// let mut agent = ReactAgent::new(config);
    /// let clients = agent.load_mcp_from_file(&path).await?;
    /// assert!(clients.is_empty());
    /// let _ = std::fs::remove_file(&path);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn load_mcp_from_file(
        &mut self,
        path: impl AsRef<std::path::Path>,
    ) -> crate::error::Result<Vec<Arc<McpClient>>> {
        let config = McpConfigFile::from_file(path)?;
        self.load_mcp_config(config).await
    }

    /// Connect every enabled server from an already parsed MCP config.
    #[cfg(feature = "mcp")]
    pub async fn load_mcp_config(
        &mut self,
        config: McpConfigFile,
    ) -> crate::error::Result<Vec<Arc<McpClient>>> {
        let server_configs = config.to_server_configs()?;
        let mut clients = Vec::new();
        for server_config in server_configs {
            let name = server_config.name.clone();
            match self.connect_mcp_from_config(server_config).await {
                Ok(client) => clients.push(client),
                Err(e) => {
                    tracing::warn!(
                        agent = %self.config.agent_name,
                        server = %name,
                        error = %e,
                        "MCP server connection failed, skipping"
                    );
                }
            }
        }
        Ok(clients)
    }

    #[cfg(feature = "mcp")]
    /// Get a connected MCP client
    ///
    /// # Parameters
    /// * `name` - MCP server name
    ///
    /// # Returns
    /// Returns the shared MCP client with the given name, or `None` if not found
    ///
    /// # Description
    /// This method retrieves a connected MCP client for direct method invocation.
    /// Clients are connected via `connect_mcp_from_config` or `load_mcp_from_file`.
    pub fn mcp_client(&self, name: &str) -> Option<Arc<McpClient>> {
        self.tools.mcp_manager.get_client(name)
    }

    #[cfg(feature = "mcp")]
    /// List all connected MCP server names
    ///
    /// # Returns
    /// List of connected MCP server names
    ///
    /// # Description
    /// This method returns the names of all currently successfully connected MCP servers,
    /// useful for displaying connection status or letting users select a specific server.
    pub fn list_mcp_servers(&self) -> Vec<String> {
        self.tools.mcp_manager.server_names()
    }

    #[cfg(feature = "mcp")]
    /// Disconnect from a specified MCP server
    ///
    /// # Parameters
    /// * `name` - Name of the MCP server to disconnect
    ///
    /// # Returns
    /// `true` if successfully disconnected, `false` if the server name was not found
    ///
    /// # Description
    /// This method disconnects from the specified MCP server and removes related tool registrations.
    /// After disconnection, tools provided by that server will no longer be available.
    pub async fn disconnect_mcp(&mut self, name: &str) -> bool {
        let tool_names = self
            .tools
            .mcp_manager
            .get_client(name)
            .map(|client| {
                client
                    .tools()
                    .iter()
                    .map(|tool| crate::mcp::McpToolAdapter::exposed_name_for(name, &tool.name))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for tool_name in tool_names {
            self.remove_tool(&tool_name);
        }
        let disconnected = self.tools.mcp_manager.disconnect(name).await;
        self.sync_mcp_resource_tools();
        self.setup_hook_mcp_executor().await;
        disconnected
    }

    // ── System Prompt ────────────────────────────────────────────────────────

    /// Set the Agent's system prompt
    ///
    /// # Parameters
    /// * `prompt` - New system prompt
    ///
    /// # Description
    /// This method updates the Agent's system prompt and synchronizes the update in the context manager.
    /// The system prompt defines the Agent's role, behavior guidelines, and task instructions.
    ///
    /// # Note
    /// Setting the system prompt overwrites all previous system prompt content.
    /// If prompts were previously injected via skills, they will also be overwritten.
    pub async fn set_system_prompt(&mut self, prompt: String) {
        self.config.system_prompt = prompt.clone();
        self.memory.context.lock().await.update_system(prompt);
        tracing::info!(
            agent = %self.config.agent_name,
            "System prompt updated"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentCallback;
    use crate::agent::config::AgentConfig;
    use crate::llm::types::Message;
    use crate::trace::RunStore;

    struct IdentifiedCallback {
        id: &'static str,
    }

    impl AgentCallback for IdentifiedCallback {
        fn callback_kind(&self) -> Option<&'static str> {
            Some("IdentifiedCallback")
        }

        fn callback_id(&self) -> Option<&str> {
            Some(self.id)
        }
    }

    #[test]
    fn remove_callbacks_by_type_name_and_id_is_precise() {
        let config = AgentConfig::minimal("test-model", "helper");
        let mut agent = ReactAgent::new(config);

        agent.add_callback(Arc::new(IdentifiedCallback { id: "task-a" }));
        agent.add_callback(Arc::new(IdentifiedCallback { id: "task-b" }));

        agent.remove_callbacks_by_type_name_and_id("IdentifiedCallback", "task-a");

        assert_eq!(agent.config.callbacks.len(), 1);
        assert_eq!(agent.config.callbacks[0].callback_id(), Some("task-b"));
    }

    #[tokio::test]
    async fn manual_compression_is_recorded_in_durable_trace() -> crate::error::Result<()> {
        let config = AgentConfig::minimal("test-model", "helper");
        let mut agent = ReactAgent::new(config);
        let store = Arc::new(crate::trace::InMemoryRunStore::new());
        agent.set_run_store(store.clone());
        let legacy = agent.capture_legacy_external_context();
        let trace_run_id = agent
            .start_legacy_trace_run("manual compress", &legacy)
            .await
            .ok_or_else(|| crate::error::ReactError::Other("trace run missing".to_string()))?;
        agent
            .memory
            .context
            .lock()
            .await
            .push(Message::user("manual compression input".to_string()));

        let _ = agent.force_compress_context().await?;
        let run = store
            .load(&trace_run_id)
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("trace data missing".to_string()))?;
        assert!(run.events.iter().any(|event| matches!(
            event,
            crate::trace::RunEvent::ContextCompression { source, .. } if source == "manual"
        )));
        Ok(())
    }
}
