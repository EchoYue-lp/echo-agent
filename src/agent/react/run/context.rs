//! Context management + long-term memory + persistence + audit

use super::super::ReactAgent;
use super::types::StreamMode;
use crate::llm::types::{Message, Role};
use crate::memory::SearchQuery;
use crate::skills::hooks::{HookContext, HookEvent};
use echo_state::memory::typed_store::{MemoryFilter, TypedMemoryStore};
use tracing::{debug, info, warn};

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

    #[cfg(feature = "human-loop")]
    pub(crate) async fn flush_pending_permission_rules(
        &self,
        service: &crate::human_loop::PermissionService,
    ) {
        let pending = match self.approval.pending_permission_rules.lock() {
            Ok(mut guard) if !guard.is_empty() => std::mem::take(&mut *guard),
            Ok(_) => return,
            Err(e) => {
                warn!("pending_permission_rules lock poisoned: {}", e);
                return;
            }
        };

        service.add_rules(pending).await;
    }

    #[allow(dead_code)]
    pub(crate) async fn log_user_input_audit(&self, content: &str) {
        if let Some(al) = &self.guard.audit_logger {
            let event = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                self.config.agent_name.clone(),
                crate::audit::AuditEventType::UserInput {
                    content: content.to_string(),
                },
            );
            if let Err(e) = al.log(event).await {
                tracing::error!(error = %e, "audit log write failed — event dropped");
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn log_tool_call_audit(
        &self,
        tool: &str,
        input: &serde_json::Value,
        output: &str,
        success: bool,
        duration_ms: u64,
    ) {
        if let Some(al) = &self.guard.audit_logger {
            let event = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                self.config.agent_name.clone(),
                crate::audit::AuditEventType::ToolCall {
                    tool: tool.to_string(),
                    input: input.clone(),
                    output: output.to_string(),
                    success,
                    duration_ms,
                },
            );
            if let Err(e) = al.log(event).await {
                tracing::error!(error = %e, "audit log write failed — event dropped");
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn log_final_answer_audit(&self, content: &str) {
        if let Some(al) = &self.guard.audit_logger {
            let event = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                self.config.agent_name.clone(),
                crate::audit::AuditEventType::FinalAnswer {
                    content: content.to_string(),
                },
            );
            if let Err(e) = al.log(event).await {
                tracing::error!(error = %e, "audit log write failed — event dropped");
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

    pub(crate) async fn restore_thread_context(&self) {
        let agent = self.config.agent_name.clone();
        let mut session_matcher = "startup";

        // Try RuntimeStateStore checkpoint (messages + plan + skills)
        if self.memory.state_store.is_some() {
            match self.resume_from_state_store().await {
                Ok(Some(_cp)) => {
                    info!(agent = %agent, "🔄 Restored from RuntimeStateStore checkpoint");
                    session_matcher = "resume";
                }
                Ok(None) => {
                    debug!(agent = %agent, "New session, starting from empty context");
                    self.reset_messages().await;
                }
                Err(e) => {
                    warn!(agent = %agent, error = %e, "⚠️ Failed to load RuntimeStateStore checkpoint, starting from empty context");
                    self.reset_messages().await;
                }
            }
        } else {
            self.reset_messages().await;
        }

        // Fire SessionStart hook with appropriate matcher
        let start_result = self
            .fire_lifecycle_hook(HookEvent::SessionStart, Some(session_matcher))
            .await;
        if start_result.block {
            warn!(agent = %self.config.agent_name, reason = ?start_result.block_reason, "SessionStart hook blocked session restore");
        }
    }

    pub(crate) async fn recall_long_term_memories(
        &self,
        query: &str,
    ) -> crate::error::Result<Vec<crate::memory::store::StoreItem>> {
        let Some(store) = &self.memory.store else {
            return Ok(vec![]);
        };
        let agent_name = self.config.agent_name.clone();
        let ns = vec![agent_name.as_str(), "memories"];

        // Search user memories
        let mut results = match store.search_with(&ns, SearchQuery::hybrid(query, 5)).await {
            Ok(items) => items,
            Err(err) if format!("{err}").contains("hybrid search") => {
                store.search(&ns, query, 5).await?
            }
            Err(err) => return Err(err),
        };

        // Also search typed layered memories written by remember, Auto Memory,
        // and background review. This is the warm layer used by
        // MemoryLayerManager, and is the main runtime recall path for new memory.
        let typed_store = TypedMemoryStore::new(store.clone());
        if let Ok(typed_items) = typed_store
            .search_typed(&["agent", "typed_memories"], query, 5, &MemoryFilter::new())
            .await
        {
            let mut existing_keys: std::collections::HashSet<String> =
                results.iter().map(|i| i.key.clone()).collect();
            for entry in typed_items {
                if existing_keys.insert(entry.key.clone()) {
                    results.push(entry.raw);
                }
            }
        }

        // Also search L3 promoted facts (compression auto-promoted memories)
        let l3_ns: &[&str] = &["l3_promoted"];
        if let Ok(l3_items) = store.search(l3_ns, query, 3).await {
            // Merge and deduplicate by key
            let existing_keys: std::collections::HashSet<String> =
                results.iter().map(|i| i.key.clone()).collect();
            for item in l3_items {
                if !existing_keys.contains(&item.key) {
                    results.push(item);
                }
            }
        }

        // Sort by score descending, limit to 5
        results.sort_by(|a, b| {
            b.score
                .unwrap_or_default()
                .partial_cmp(&a.score.unwrap_or_default())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(5);

        Ok(results)
    }

    pub(crate) async fn inject_hook_messages(
        &self,
        source: &str,
        phase: &str,
        identifier: &str,
        messages: &[String],
    ) {
        let mut ctx = self.memory.context.lock().await;
        for message in messages {
            ctx.push(Message::system(format!(
                "[{source}:{phase}:{identifier}]\n{message}"
            )));
        }
    }

    pub(crate) async fn apply_hook_messages(
        &self,
        tool_name: &str,
        hook_messages: &HookMessageBatches,
    ) {
        self.inject_hook_messages("Hook", "PreToolUse", tool_name, &hook_messages.pre)
            .await;
        self.inject_hook_messages("Hook", "PostToolUse", tool_name, &hook_messages.post)
            .await;
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
                // prompt is set by caller, use empty here
                HookContext::for_user_prompt_submit("", None, &session_id, &agent_name)
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
                &session_id,
                &agent_name,
            ),
            HookEvent::TaskCreated => {
                HookContext::for_task_created("", matcher.unwrap_or(""), &session_id, &agent_name)
            }
            HookEvent::TaskCompleted => HookContext::for_task_completed(
                "",
                matcher.unwrap_or(""),
                "",
                &session_id,
                &agent_name,
            ),
            // New events — use generic lifecycle context
            HookEvent::PluginLoaded | HookEvent::PluginDisabled => {
                HookContext::for_lifecycle(event, matcher.unwrap_or(""), &session_id, &agent_name)
            }
            HookEvent::TaskTimeout | HookEvent::TaskCancelled => {
                HookContext::for_lifecycle(event, matcher.unwrap_or(""), &session_id, &agent_name)
            }
            HookEvent::SubagentCancelled => {
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
        let result = registry.run_lifecycle_hooks(&context).await;

        // Inject context from hook results (single lock acquisition for batching)
        let event_name = event.as_str();
        if result.injected_context.is_some() || !result.messages.is_empty() {
            let mut ctx = self.memory.context.lock().await;
            if let Some(ctx_text) = &result.injected_context {
                ctx.push(Message::system(format!(
                    "[Hook:{}] {}",
                    event_name, ctx_text
                )));
            }
            for msg in &result.messages {
                ctx.push(Message::system(format!("[Hook:{}] {}", event_name, msg)));
            }
        }

        result
    }

    /// Common initialization logic for streaming execution
    ///
    /// Decides whether to reset context or restore from checkpoint based on the mode.
    /// Returns the number of recalled long-term memories (0 means no memories were injected).
    pub(crate) async fn prepare_stream_context(&self, mode: StreamMode, input: &str) -> usize {
        match mode {
            StreamMode::Execute => {
                self.restore_thread_context().await;
            }
            StreamMode::Chat => {
                // Multi-turn chat mode: do not reset context
            }
        }

        self.detect_and_write_memory_triggers(input).await;

        // Inject relevant long-term memories
        let mut recalled = 0usize;
        if let Ok(items) = self.recall_long_term_memories(input).await
            && !items.is_empty()
        {
            recalled = items.len();
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
                "[The above memories are for reference; answer the user's CURRENT question.]"
                    .to_string(),
            );
            // Inject as a system message — long-term memories are background context,
            // not new user input. Pushing them as `Message::user` produced two
            // consecutive user turns and confused the model into treating the
            // recalled memory as a fresh request.
            self.memory
                .context
                .lock()
                .await
                .push(Message::system(lines.join("\n")));
        }

        // Push user message
        self.memory
            .context
            .lock()
            .await
            .push(Message::user(input.to_string()));
        recalled
    }

    /// Streaming execution context initialization (multimodal message version)
    ///
    /// Same as `prepare_stream_context`, but accepts a pre-built `Message` instead of a string,
    /// supporting multimodal content parts (images, files, etc.).
    pub(crate) async fn prepare_stream_context_with_message(
        &self,
        mode: StreamMode,
        message: &Message,
    ) -> usize {
        match mode {
            StreamMode::Execute => {
                self.restore_thread_context().await;
            }
            StreamMode::Chat => {}
        }

        // Extract text from message for long-term memory retrieval
        let text = message.content.as_text().unwrap_or_default();
        if !text.is_empty() {
            self.detect_and_write_memory_triggers(&text).await;
        }

        let mut recalled = 0usize;
        if !text.is_empty()
            && let Ok(items) = self.recall_long_term_memories(&text).await
            && !items.is_empty()
        {
            recalled = items.len();
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
                "[The above memories are for reference; answer the user's CURRENT question.]"
                    .to_string(),
            );
            // System role — see prepare_stream_context for rationale.
            self.memory
                .context
                .lock()
                .await
                .push(Message::system(lines.join("\n")));
        }

        // Push multimodal user message
        self.memory.context.lock().await.push(message.clone());
        recalled
    }
}
