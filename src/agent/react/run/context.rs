//! Context management + long-term memory + persistence + audit

use super::super::ReactAgent;
use super::types::StreamMode;
use crate::llm::types::{Message, Role};
use crate::skills::hooks::{HookContext, HookEvent};
use echo_core::memory::types::{MemoryMeta, MemorySource, MemoryType};
use tracing::{debug, info, warn};

pub(crate) const TURN_MEMORY_CONTEXT_PROJECTION: &str = "echo-agent:turn-memory-context";
pub(crate) const WORKSPACE_CONTEXT_PROJECTION: &str = "echo-agent:workspace-context";

/// (stage4 E1) Prompt for the pre-compaction flush LLM call.
const PRE_COMPACTION_FLUSH_PROMPT: &str = "\
You are a memory-compaction flusher. The conversation below is about to be \
compressed (older messages will be summarized away). Identify DURABLE facts \
worth persisting long-term: stable user preferences, project facts, \
architecture decisions, debugging lessons, verified error resolutions. Do NOT \
capture transient/session-specific details or task narratives.\n\
Reply with a JSON array of items to persist, each: \
{\"content\": \"<concise fact>\", \"type\": \"user_preference|project_fact|architecture_decision|debugging_lesson|error_resolution|command_pattern|tool_usage\", \"recall_weight\": <0.0-1.0>}.\n\
If nothing is durable, reply exactly: NO_REPLY";

#[derive(Clone, Default)]
pub(crate) struct HookMessageBatches {
    pub pre: Vec<String>,
    pub post: Vec<String>,
}

impl ReactAgent {
    /// Detect memory-worthy conversational triggers and persist them through the
    /// real layered memory manager. Currently this wires the reliable
    /// user-correction path; explicit saves are handled by the `remember` tool.
    pub(crate) async fn detect_and_write_memory_triggers(&self, user_message: &str) {
        let Some(layer_manager) = &self.memory_layer_manager else {
            return;
        };

        let assistant_message = {
            let ctx = self.memory.context.lock().await;
            ctx.messages()
                .iter()
                .rev()
                .find(|m| matches!(m.role, Role::Assistant))
                .and_then(|m| m.content.as_text())
                .map(|s| s.to_string())
        };
        let (last_tool_failure, last_tool_success, tool_sequences) =
            match self.memory_trigger_state.lock() {
                Ok(state) => (
                    state.last_tool_failure.clone(),
                    state.last_tool_success.clone(),
                    state.tool_sequences.clone(),
                ),
                Err(e) => {
                    tracing::warn!("memory trigger state lock poisoned: {}", e);
                    (None, None, Vec::new())
                }
            };

        let trigger_ctx = crate::evolution::TriggerContext {
            user_message: Some(user_message.to_string()),
            assistant_message,
            last_tool_failure,
            last_tool_success,
            tool_sequences,
            ..Default::default()
        };
        let detector = crate::evolution::TriggerDetector::new();
        let triggers = detector.detect(&trigger_ctx);
        let mut consumed_error_resolution = false;
        let mut consumed_repeated_workflow = false;
        for trigger in triggers {
            if let Some(sink) = &self.memory_trigger_sink {
                match sink.on_trigger(&trigger).await {
                    Ok(crate::evolution::MemoryTriggerDisposition::Captured) => {
                        consumed_error_resolution |=
                            matches!(trigger.source, crate::memory::MemorySource::ErrorResolution);
                        consumed_repeated_workflow |= matches!(
                            trigger.source,
                            crate::memory::MemorySource::RepeatedWorkflow
                        );
                        continue;
                    }
                    Ok(crate::evolution::MemoryTriggerDisposition::Persist) => {}
                    Err(error) => {
                        tracing::warn!(
                            key = %trigger.suggested_key,
                            %error,
                            "Memory trigger sink failed; falling back to durable write"
                        );
                    }
                }
            }

            let meta =
                crate::memory::MemoryMeta::new(trigger.memory_type, trigger.source, trigger.topic)
                    .with_confidence(trigger.confidence);

            match layer_manager
                .write_memory(&trigger.suggested_key, &trigger.content, meta)
                .await
            {
                Ok(_) => {
                    consumed_error_resolution |=
                        matches!(trigger.source, crate::memory::MemorySource::ErrorResolution);
                    consumed_repeated_workflow |= matches!(
                        trigger.source,
                        crate::memory::MemorySource::RepeatedWorkflow
                    );
                    tracing::info!(
                        key = %trigger.suggested_key,
                        "Memory trigger persisted through layered memory"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        key = %trigger.suggested_key,
                        error = %e,
                        "Memory trigger write failed"
                    );
                }
            }
        }

        if (consumed_error_resolution || consumed_repeated_workflow)
            && let Ok(mut state) = self.memory_trigger_state.lock()
        {
            if consumed_error_resolution {
                state.last_tool_failure = None;
                state.last_tool_success = None;
            }
            if consumed_repeated_workflow {
                state.tool_sequences.clear();
            }
        }
    }

    /// Reset message history, keeping only the system prompt to ensure each execution is independent
    pub(crate) async fn reset_messages(&self) {
        let mut ctx = self.memory.context.lock().await;
        ctx.clear();
        ctx.push(Message::system(self.config.system_prompt.clone()));
        drop(ctx);
        // Fire SessionStart hook with matcher "clear"
        let start_result = self
            .fire_lifecycle_hook(HookEvent::SessionStart, Some("clear"))
            .await;
        if start_result.block {
            warn!(agent = %self.config.agent_name, reason = ?start_result.block_reason, "SessionStart hook blocked new session");
        }
    }

    async fn reset_runtime_context(&self) {
        self.reset_messages().await;
        *self.plan_state.write().await = None;
        self.tools.skill_registry.reset_activation_state();
        if let Some(registry) = &self.tools.progressive_skill_registry {
            registry.write().await.reset_activation_state();
        }
        *self.memory.transcript_projection_cursor.lock().await =
            crate::agent::snapshot::TranscriptProjectionCursor::default();
        self.set_working_dir(self.memory.configured_working_dir.clone());
    }

    pub(crate) async fn begin_runtime_state_hydration(
        &self,
        runtime_state_id: Option<&str>,
    ) -> super::super::subsystems::memory::RuntimeStateHydration {
        let mut hydration = self.memory.runtime_state_hydration.lock().await;
        std::mem::replace(
            &mut *hydration,
            super::super::subsystems::memory::RuntimeStateHydration::Hydrating(
                runtime_state_id.map(str::to_string),
            ),
        )
    }

    pub(crate) async fn commit_runtime_state_hydration(&self, runtime_state_id: Option<&str>) {
        let mut hydration = self.memory.runtime_state_hydration.lock().await;
        *hydration = super::super::subsystems::memory::RuntimeStateHydration::Hydrated(
            runtime_state_id.map(str::to_string),
        );
    }

    pub(crate) fn clear_runtime_snapshots(&self) {
        let mut snapshots = match self.memory.snapshot_manager.write() {
            Ok(snapshots) => snapshots,
            Err(poisoned) => {
                tracing::error!("runtime snapshot manager lock was poisoned");
                poisoned.into_inner()
            }
        };
        if let Some(manager) = snapshots.as_mut() {
            manager.clear();
        }
    }

    pub(crate) async fn restore_thread_context(&self) -> crate::error::Result<()> {
        let runtime_state_id = self.config.conversation_id.clone();
        self.restore_thread_context_for(runtime_state_id.as_deref())
            .await
    }

    async fn restore_thread_context_for(
        &self,
        runtime_state_id: Option<&str>,
    ) -> crate::error::Result<()> {
        let _previous_hydration = self.begin_runtime_state_hydration(runtime_state_id).await;
        self.clear_runtime_snapshots();
        let agent = self.config.agent_name.clone();
        let mut session_matcher = "startup";

        // Try RuntimeStateStore checkpoint (messages + plan + skills)
        if self.memory.state_store.is_some() {
            let restored = match runtime_state_id {
                Some(runtime_state_id) => self.resume_from_state_store_id(runtime_state_id).await,
                None => Ok(None),
            };
            match restored {
                Ok(Some(_cp)) => {
                    info!(agent = %agent, "🔄 Restored from RuntimeStateStore checkpoint");
                    session_matcher = "resume";
                }
                Ok(None) => {
                    debug!(agent = %agent, "New session, starting from empty context");
                    self.reset_runtime_context().await;
                }
                Err(e) => {
                    return Err(crate::error::ReactError::Other(format!(
                        "Runtime checkpoint recovery failed for agent '{agent}': {e}"
                    )));
                }
            }
        } else {
            self.reset_runtime_context().await;
        }

        // Fire SessionStart hook with appropriate matcher
        let start_result = self
            .fire_lifecycle_hook(HookEvent::SessionStart, Some(session_matcher))
            .await;
        if start_result.block {
            warn!(agent = %self.config.agent_name, reason = ?start_result.block_reason, "SessionStart hook blocked session restore");
        }
        self.commit_runtime_state_hydration(runtime_state_id).await;
        Ok(())
    }

    /// Restore a persisted chat exactly once on a cold agent instance.
    /// Existing in-process history is authoritative and must never be replaced
    /// between turns.
    pub(crate) async fn restore_chat_context_if_cold(&self) -> crate::error::Result<()> {
        let runtime_state_id = self.config.conversation_id.clone();
        self.restore_chat_context_if_cold_for(runtime_state_id.as_deref())
            .await
    }

    async fn restore_chat_context_if_cold_for(
        &self,
        runtime_state_id: Option<&str>,
    ) -> crate::error::Result<()> {
        let is_cold = {
            let context = self.memory.context.lock().await;
            context
                .messages()
                .iter()
                .all(|message| matches!(message.role, Role::System))
        };
        let effective_runtime_state_id = runtime_state_id.map(str::to_string);
        let should_restore = {
            let mut hydration = self.memory.runtime_state_hydration.lock().await;
            match &*hydration {
                super::super::subsystems::memory::RuntimeStateHydration::Hydrated(current)
                    if current == &effective_runtime_state_id =>
                {
                    is_cold
                }
                super::super::subsystems::memory::RuntimeStateHydration::Uninitialized
                    if !is_cold && effective_runtime_state_id == self.config.conversation_id =>
                {
                    *hydration = super::super::subsystems::memory::RuntimeStateHydration::Hydrated(
                        effective_runtime_state_id.clone(),
                    );
                    false
                }
                _ => true,
            }
        };
        if should_restore {
            self.restore_thread_context_for(runtime_state_id).await?;
        }
        Ok(())
    }

    /// (stage4 D1) Unified recall — delegates to `MemoryRecaller` so the auto
    /// recall path and the tool recall path (`LayeredRecallTool`) share one
    /// composite-score entry over the unified namespace (割裂点 3/9).
    pub(crate) async fn recall_long_term_memories(
        &self,
        query: &str,
    ) -> crate::error::Result<Vec<crate::memory::store::StoreItem>> {
        let Some(store) = &self.memory.store else {
            return Ok(vec![]);
        };
        let reca = crate::evolution::recall::MemoryRecaller::new(store.clone());
        reca.recall(query, 5).await
    }

    /// Fire a lifecycle hook and inject any results into the agent context.
    pub async fn fire_lifecycle_hook(
        &self,
        event: HookEvent,
        matcher: Option<&str>,
    ) -> crate::skills::hooks::HookResult {
        let session_id = self.config.session_id.clone().unwrap_or_default();
        let agent_name = self.config.agent_name.clone();

        let context = match event {
            HookEvent::SessionStart => HookContext::for_session_start(
                matcher.unwrap_or("startup"),
                &session_id,
                &agent_name,
            ),
            HookEvent::SessionEnd => {
                HookContext::for_session_end(matcher.unwrap_or("other"), &session_id, &agent_name)
            }
            HookEvent::Stop => HookContext::for_stop(None, &session_id, &agent_name, false),
            HookEvent::Notification => {
                HookContext::for_notification(matcher.unwrap_or(""), &session_id, &agent_name)
            }
            HookEvent::UserPromptSubmit => {
                // User input text is passed via the `matcher` parameter by the
                // caller (prepare_stream_context / prepare_react_context).
                // This enables content-based hook matchers (e.g. "\\.docx")
                // to match against the actual user input via glob::Pattern.
                let prompt_text = matcher.unwrap_or("");
                HookContext::for_user_prompt_submit(prompt_text, matcher, &session_id, &agent_name)
            }
            HookEvent::PreCompact | HookEvent::PostCompact => {
                // Compress stats are set by caller, use default here
                let stats = Default::default();
                if event == HookEvent::PreCompact {
                    HookContext::for_pre_compact(
                        &stats,
                        matcher.unwrap_or("auto"),
                        &session_id,
                        &agent_name,
                    )
                } else {
                    HookContext::for_post_compact(
                        &stats,
                        matcher.unwrap_or("auto"),
                        &session_id,
                        &agent_name,
                    )
                }
            }
            HookEvent::ConfigChange => HookContext::for_config_change(
                matcher.unwrap_or(""),
                matcher,
                &session_id,
                &agent_name,
            ),
            HookEvent::InstructionsLoaded => HookContext::for_instructions_loaded(
                matcher.unwrap_or("startup"),
                &[],
                &session_id,
                &agent_name,
            ),
            HookEvent::PostToolBatch => {
                // Batch details are set by caller; use defaults here
                HookContext::for_post_tool_batch(&[], 0, 0, &session_id, &agent_name)
            }
            HookEvent::StopFailure => {
                HookContext::for_stop_failure("", matcher.unwrap_or(""), &session_id, &agent_name)
            }
            HookEvent::SubagentStart => HookContext::for_subagent_start(
                "",
                "",
                matcher.unwrap_or(""),
                &session_id,
                &agent_name,
            ),
            HookEvent::SubagentStop => HookContext::for_subagent_stop(
                "",
                "",
                matcher.unwrap_or(""),
                // Fallback mapping lacks a real terminal status; callers that
                // fire SubagentStop directly pass the actual status. Default
                // to Completed so this placeholder compiles without guessing.
                echo_core::hooks::SubagentStopStatus::Completed,
                &session_id,
                &agent_name,
            ),
            HookEvent::TaskCreated => {
                HookContext::for_task_created("", matcher.unwrap_or(""), &session_id, &agent_name)
            }
            HookEvent::TaskStarted => {
                HookContext::for_task_started("", matcher.unwrap_or(""), &session_id, &agent_name)
            }
            HookEvent::TaskCompleted => HookContext::for_task_completed(
                "",
                matcher.unwrap_or(""),
                "",
                // Fallback mapping lacks a real terminal status; direct
                // callers pass the actual status. Default Completed so this
                // placeholder compiles without guessing.
                echo_core::hooks::TaskTerminalStatus::Completed,
                &session_id,
                &agent_name,
            ),
            // New events — use generic lifecycle context
            HookEvent::PluginLoaded | HookEvent::PluginDisabled => {
                HookContext::for_lifecycle(event, matcher.unwrap_or(""), &session_id, &agent_name)
            }
            // Evolution events — use dedicated factory methods
            HookEvent::PostMemoryWrite => HookContext::for_post_memory_write(
                "",
                matcher.unwrap_or(""),
                &session_id,
                &agent_name,
            ),
            HookEvent::MemoryLayerChange => HookContext::for_memory_layer_change(
                "",
                "",
                matcher.unwrap_or(""),
                &session_id,
                &agent_name,
            ),
            // Phase 5 evolution events — use generic lifecycle context
            HookEvent::SkillCandidateDetected
            | HookEvent::SkillLifecycleTransition
            | HookEvent::SkillHealthCheck
            | HookEvent::SkillPatchApplied
            | HookEvent::SkillMergeApplied
            | HookEvent::RulePromoted => {
                HookContext::for_lifecycle(event, matcher.unwrap_or(""), &session_id, &agent_name)
            }
            // Tool events should use run_pre_tool_use / run_post_tool_use / run_post_tool_use_failure directly
            HookEvent::PreToolUse
            | HookEvent::PostToolUse
            | HookEvent::PostToolUseFailure
            | HookEvent::PermissionRequest
            | HookEvent::PermissionDenied => {
                warn!(event = ?event, "Tool event dispatched via fire_lifecycle_hook; use dedicated tool hook methods instead");
                return crate::skills::hooks::HookResult::default();
            }
        };

        let registry = self.tools.hook_registry.read().await.clone();
        let mut result = registry.run_lifecycle_hooks(&context).await;

        // Inject context from hook results (single lock acquisition for batching)
        let event_name = event.as_str();
        if result.injected_context.is_some() || !result.messages.is_empty() {
            let mut ctx = self.memory.context.lock().await;
            if let Some(ctx_text) = &result.injected_context {
                ctx.push(runtime_context_note(
                    &format!("Hook:{event_name}"),
                    ctx_text,
                ));
            }
            for msg in &result.messages {
                ctx.push(runtime_context_note(&format!("Hook:{event_name}"), msg));
            }
        }

        // ActivateSkill hook: directly activate the requested skill.
        // On success, clear the field so callers (prepare phase → cache writer)
        // don't also hand it to TriggerSupervisor for a double activation.
        if let Some((ref skill, ref reason)) = result.activate_skill {
            match self.activate_skill(skill).await {
                Ok(()) => {
                    let note = format!("已根据上下文自动激活技能 {skill}:{reason}");
                    let mut ctx = self.memory.context.lock().await;
                    ctx.push(runtime_context_note("Hook:ActivateSkill", &note));
                    info!(skill = %skill, reason = %reason, "Hook activated skill");
                    // Consumed — prevent double activation by supervisor (P4)
                    result.activate_skill = None;
                }
                Err(e) => {
                    warn!(skill = %skill, error = %e, "Hook-requested skill activation failed");
                    // Leave activate_skill in result so supervisor (P4) can retry
                }
            }
        }

        result
    }

    /// Common initialization logic for streaming execution
    ///
    /// Decides whether to reset context or restore from checkpoint based on the mode.
    /// Returns the number of recalled long-term memories (0 means no memories were injected).
    pub(crate) async fn prepare_stream_context(
        &self,
        mode: StreamMode,
        input: &str,
        history: &[Message],
        runtime_state_id: Option<&str>,
    ) -> crate::error::Result<usize> {
        // Clear read-before-edit tracking for the new conversation turn
        // (converged with prepare_react_context; the entry layer no longer
        // clears it separately to avoid a double clear).
        self.clear_read_files();
        match mode {
            StreamMode::Execute => {
                self.restore_thread_context_for(runtime_state_id).await?;
            }
            StreamMode::Chat => {
                self.restore_chat_context_if_cold_for(runtime_state_id)
                    .await?;
            }
        }

        self.detect_and_write_memory_triggers(input).await;

        // Recall relevant long-term memories for this turn.
        let mut recalled = 0usize;
        let mut memory_context = None;
        if let Ok(items) = self.recall_long_term_memories(input).await
            && !items.is_empty()
        {
            recalled = items.len();
            memory_context = Some(format_memory_context(&items));
        }

        let wd = self.config.working_dir.lock().ok().and_then(|g| g.clone());
        let ws_block = crate::agent::react::ReactAgent::build_workspace_context_block(wd.as_ref());

        let mut context = self.memory.context.lock().await;
        context.replace_projection(
            WORKSPACE_CONTEXT_PROJECTION,
            (!ws_block.trim().is_empty()).then(|| runtime_context_note("workspace", &ws_block)),
        );
        context.replace_tail_projection(
            TURN_MEMORY_CONTEXT_PROJECTION,
            memory_context.map(|body| runtime_context_note("memory", body.as_str())),
        );
        for message in history {
            context.push(message.clone());
        }
        context.push(Message::user(input.to_string()));
        // Drop context lock before hook execution (avoid deadlock with
        // fire_lifecycle_hook's own context acquisition)
        drop(context);

        // Fire UserPromptSubmit hook: passes user input so content-based
        // matchers (e.g. "\\.docx") can trigger ActivateSkill actions, and
        // static "*" matchers inject the forced-checklist prompt every turn.
        let hook_result = self
            .fire_lifecycle_hook(HookEvent::UserPromptSubmit, Some(input))
            .await;
        // Cache hook activation result for TriggerSupervisor (P4) consumption
        if hook_result.activate_skill.is_some()
            && let Ok(mut cache) = self.hook_activation_cache.lock()
        {
            *cache = hook_result.activate_skill;
        }

        Ok(recalled)
    }

    /// Streaming execution context initialization (multimodal message version)
    ///
    /// Same as `prepare_stream_context`, but accepts a pre-built `Message` instead of a string,
    /// supporting multimodal content parts (images, files, etc.).
    pub(crate) async fn prepare_stream_context_with_message(
        &self,
        mode: StreamMode,
        message: &Message,
        history: &[Message],
        runtime_state_id: Option<&str>,
    ) -> crate::error::Result<usize> {
        // Clear read-before-edit tracking (see prepare_stream_context).
        self.clear_read_files();
        match mode {
            StreamMode::Execute => {
                self.restore_thread_context_for(runtime_state_id).await?;
            }
            StreamMode::Chat => {
                self.restore_chat_context_if_cold_for(runtime_state_id)
                    .await?
            }
        }

        // Extract text from message for long-term memory retrieval
        let text = message.content.as_text().unwrap_or_default();
        if !text.is_empty() {
            self.detect_and_write_memory_triggers(&text).await;
        }

        let mut recalled = 0usize;
        let mut memory_context = None;
        if !text.is_empty()
            && let Ok(items) = self.recall_long_term_memories(&text).await
            && !items.is_empty()
        {
            recalled = items.len();
            memory_context = Some(format_memory_context(&items));
        }

        let wd = self.config.working_dir.lock().ok().and_then(|g| g.clone());
        let ws_block = crate::agent::react::ReactAgent::build_workspace_context_block(wd.as_ref());

        let mut context = self.memory.context.lock().await;
        context.replace_projection(
            WORKSPACE_CONTEXT_PROJECTION,
            (!ws_block.trim().is_empty()).then(|| runtime_context_note("workspace", &ws_block)),
        );
        context.replace_tail_projection(
            TURN_MEMORY_CONTEXT_PROJECTION,
            memory_context.map(|body| runtime_context_note("memory", body.as_str())),
        );
        for history_message in history {
            context.push(history_message.clone());
        }
        context.push(message.clone());
        drop(context);

        // Fire UserPromptSubmit hook (see prepare_stream_context for rationale)
        if !text.is_empty() {
            let hook_result = self
                .fire_lifecycle_hook(HookEvent::UserPromptSubmit, Some(&text))
                .await;
            if hook_result.activate_skill.is_some()
                && let Ok(mut cache) = self.hook_activation_cache.lock()
            {
                *cache = hook_result.activate_skill;
            }
        }

        Ok(recalled)
    }
}

pub(crate) async fn push_runtime_context_note(
    context: &tokio::sync::Mutex<crate::compression::ContextManager>,
    source: &str,
    body: &str,
) {
    context
        .lock()
        .await
        .push(runtime_context_note(source, body));
}

pub(crate) fn runtime_context_note(source: &str, body: &str) -> Message {
    Message::user(format!(
        "[runtime_context:{source}]\n{body}\n[Use this runtime context to continue the current task. It is dynamic turn state, not stable system policy.]"
    ))
}

pub(crate) fn format_memory_context(items: &[crate::memory::store::StoreItem]) -> String {
    let mut lines = vec!["[memory_context] Relevant historical memories:".to_string()];
    for (i, item) in items.iter().enumerate() {
        let content_str = item
            .value
            .get("content")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| item.value.to_string());
        lines.push(format!("{}. {}", i + 1, content_str));
    }
    lines.push(
        "[The above memories are for reference; answer the user's CURRENT question.]".to_string(),
    );
    lines.join("\n")
}

// (stage4 D1) recall helpers (composite_score / age_days_from_storeitem /
// incr_recall_count) moved to `evolution::recall::MemoryRecaller`.

impl crate::agent::snapshot::AgentRunSnapshot {
    /// (stage4 E1) Pre-compaction flush — a bounded LLM call identifies durable
    /// rules/skills/facts in the about-to-be-compressed conversation and persists
    /// them to the unified memory store so they survive compaction (割裂点 1/7).
    ///
    /// Approach: pragmatic structured-return flush — the LLM decides what's
    /// durable and returns a JSON array; the framework writes each item via the
    /// shared `MemoryLayerManager` (typed memory, security checks, audit hooks).
    /// This is the user-chosen variant (overrides D14-G1's "真 subagent" — a
    /// subagent fork was judged higher-cost for equivalent durable-extraction
    /// value on a local single-user assistant). Best-effort — errors/timeouts
    /// never block compaction. Gated by `ContextManager::should_compress()` so it
    /// only fires when compaction is imminent, not every ReAct iteration.
    pub(crate) async fn pre_compaction_flush(
        &self,
        context: &std::sync::Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
    ) {
        let (Some(llm_client), Some(layer_manager)) =
            (self.llm_client.as_ref(), self.memory_layer_manager.as_ref())
        else {
            return; // no LLM or no layer manager — skip
        };

        // Snapshot the about-to-be-compressed messages (last ~40, chronological).
        let transcript = {
            let ctx = context.lock().await;
            // (stage4 E1) Only flush when compression is imminent — mirrors
            // `ContextManager::prepare`'s `needs_compression` decision. Avoids
            // firing an LLM call every ReAct iteration when no compaction is due.
            if !ctx.should_compress() {
                return;
            }
            let msgs = ctx.messages();
            if msgs.len() < 8 {
                return; // too short to warrant a flush even if tokens say so
            }
            let mut buf = String::new();
            for m in msgs
                .iter()
                .rev()
                .take(40)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                buf.push_str(&format!(
                    "[{}] {}\n",
                    m.role.as_str(),
                    m.content.as_text_ref().unwrap_or("")
                ));
            }
            buf
        };
        if transcript.trim().is_empty() {
            return;
        }

        let request = crate::llm::ChatRequest {
            messages: vec![
                Message::system(PRE_COMPACTION_FLUSH_PROMPT.to_string()),
                Message::user(transcript),
            ],
            temperature: Some(0.2),
            max_tokens: Some(2048),
            user_id: self.config.cache_user_id.clone(),
            ..Default::default()
        };

        // Bounded 15s; errors/timeouts never block compaction.
        let response = match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            llm_client.chat(request),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "pre_compaction_flush LLM call failed");
                return;
            }
            Err(_) => {
                tracing::debug!("pre_compaction_flush timed out (15s)");
                return;
            }
        };
        let content = response.content().unwrap_or_default();
        if content.trim().is_empty() || content.trim().eq_ignore_ascii_case("no_reply") {
            return;
        }

        // Parse a JSON array of {content, type, recall_weight}; write each.
        let items: Vec<serde_json::Value> = match (content.find('['), content.rfind(']')) {
            (Some(start), Some(end)) if end > start => content
                .get(start..=end)
                .and_then(|json| serde_json::from_str(json).ok())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for item in items {
            let Some(fact) = item.get("content").and_then(|v| v.as_str()) else {
                continue;
            };
            let ty = item
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("project_fact");
            let rw = item
                .get("recall_weight")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.5)
                .clamp(0.0, 1.0);
            let memory_type = match ty.to_lowercase().as_str() {
                "user_preference" => MemoryType::UserPreference,
                "project_fact" => MemoryType::ProjectFact,
                "architecture_decision" => MemoryType::ArchitectureDecision,
                "debugging_lesson" => MemoryType::DebuggingLesson,
                "error_resolution" => MemoryType::ErrorResolution,
                "command_pattern" => MemoryType::CommandPattern,
                "tool_usage" => MemoryType::ToolUsage,
                _ => MemoryType::ProjectFact,
            };
            let meta = MemoryMeta::new(memory_type, MemorySource::L3Promotion, "compaction_flush")
                .with_recall_weight(rw as f32);
            let key = crate::memory_promoter::durable_memory_content_key(fact);
            if layer_manager
                .locate(&key)
                .await
                .is_some_and(|(_, existing)| existing.content.trim() == fact.trim())
            {
                continue;
            }
            if let Err(e) = layer_manager.write_memory(&key, fact, meta).await {
                tracing::debug!(error = %e, "pre_compaction_flush write_memory failed");
            }
        }
    }
}
