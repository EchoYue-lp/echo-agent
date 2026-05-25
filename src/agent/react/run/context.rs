//! Context management + long-term memory + persistence + audit

use super::super::ReactAgent;
use super::types::StreamMode;
use crate::llm::types::Message;
use crate::memory::conversation::project_messages;
use crate::memory::{NewConversation, SearchQuery, ThreadState};
use crate::skills::hooks::{HookContext, HookEvent};
use tracing::{debug, info, warn};

#[derive(Clone, Default)]
pub(crate) struct HookMessageBatches {
    pub pre: Vec<String>,
    pub post: Vec<String>,
}

impl ReactAgent {
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

    /// Automatically capture a state snapshot according to the snapshot policy
    pub(crate) async fn auto_snapshot(&self, iteration: usize) {
        let should = self
            .memory
            .snapshot_manager
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|mgr| mgr.should_capture(iteration));

        if should {
            let ctx = self.memory.context.lock().await;
            let messages = ctx.messages().to_vec();
            let id = self
                .memory
                .snapshot_manager
                .write()
                .unwrap()
                .as_mut()
                .unwrap()
                .capture(iteration, &messages);
            debug!(
                agent = %self.config.agent_name,
                iteration = iteration,
                snapshot_id = %id,
                "📸 Auto-snapshot captured"
            );
        }
    }

    pub(crate) async fn log_user_input_audit(&self, content: &str) {
        if let Some(al) = &self.guard.audit_logger {
            let event = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                self.config.agent_name.clone(),
                crate::audit::AuditEventType::UserInput {
                    content: content.to_string(),
                },
            );
            let _ = al.log(event).await;
        }
    }

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
            let _ = al.log(event).await;
        }
    }

    pub(crate) async fn log_final_answer_audit(&self, content: &str) {
        if let Some(al) = &self.guard.audit_logger {
            let event = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                self.config.agent_name.clone(),
                crate::audit::AuditEventType::FinalAnswer {
                    content: content.to_string(),
                },
            );
            let _ = al.log(event).await;
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
        let session_matcher;
        if let (Some(cp), Some(tid)) = (&self.memory.checkpointer, &self.config.session_id) {
            match cp.get_state(tid).await {
                Ok(Some(state)) => {
                    info!(agent = %agent, session_id = %tid, "🔄 Restoring session from thread state");
                    self.memory
                        .context
                        .lock()
                        .await
                        .set_messages(state.messages);
                    session_matcher = "resume";
                }
                Ok(None) => {
                    debug!(agent = %agent, session_id = %tid, "New session, starting from empty context");
                    self.reset_messages().await;
                    return; // reset_messages already fires SessionStart("clear")
                }
                Err(e) => {
                    warn!(agent = %agent, error = %e, "⚠️ Failed to load thread state, starting from empty context");
                    self.reset_messages().await;
                    return;
                }
            }
        } else {
            self.reset_messages().await;
            return;
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
        match store.search_with(&ns, SearchQuery::hybrid(query, 5)).await {
            Ok(items) => Ok(items),
            Err(err) if format!("{err}").contains("hybrid search") => {
                store.search(&ns, query, 5).await
            }
            Err(err) => Err(err),
        }
    }

    pub(crate) async fn sync_conversation_projection(&self) {
        let Some(store) = &self.memory.conversation_store else {
            return;
        };
        let Some(conversation_id) = self.config.get_conversation_id() else {
            warn!(
                agent = %self.config.agent_name,
                "⚠️ ConversationStore configured but conversation_id is missing, skipping history projection"
            );
            return;
        };

        let new_conversation = NewConversation {
            conversation_id: conversation_id.to_string(),
            user_id: "default".to_string(),
            agent_type: Some("react".to_string()),
            title: None,
        };

        let messages = {
            let ctx = self.memory.context.lock().await;
            ctx.messages().to_vec()
        };

        let result = async {
            store.ensure_conversation(new_conversation).await?;
            let projected = project_messages(conversation_id, &messages)?;
            store.save_messages(conversation_id, &projected).await
        }
        .await;

        if let Err(e) = result {
            warn!(
                agent = %self.config.agent_name,
                conversation_id = %conversation_id,
                error = %e,
                "⚠️ Conversation history projection save failed"
            );
        }
    }

    pub(crate) async fn persist_runtime_state(&self) {
        if let (Some(cp), Some(tid)) = (&self.memory.checkpointer, self.config.session_id.clone()) {
            let messages = {
                let ctx = self.memory.context.lock().await;
                ctx.messages().to_vec()
            };
            let state = ThreadState::from_messages(messages);
            match cp.put_state(&tid, state).await {
                Ok(cid) => {
                    debug!(agent = %self.config.agent_name, session_id = %tid, checkpoint_id = %cid, "🔖 Thread state saved")
                }
                Err(e) => {
                    warn!(agent = %self.config.agent_name, error = %e, "⚠️ Thread state save failed")
                }
            }
        }
        self.sync_conversation_projection().await;
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

        // Inject relevant long-term memories
        let mut recalled = 0usize;
        if let Ok(items) = self.recall_long_term_memories(input).await
            && !items.is_empty()
        {
            recalled = items.len();
            let mut lines = vec!["[Relevant historical memories]".to_string()];
            for (i, item) in items.iter().enumerate() {
                let content_str = item
                    .value
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| item.value.to_string());
                lines.push(format!("{}. {}", i + 1, content_str));
            }
            lines.push("[The above memories are for reference, please answer with the current question in mind]".to_string());
            self.memory
                .context
                .lock()
                .await
                .push(Message::user(lines.join("\n")));
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
        let mut recalled = 0usize;
        if !text.is_empty()
            && let Ok(items) = self.recall_long_term_memories(&text).await
            && !items.is_empty()
        {
            recalled = items.len();
            let mut lines = vec!["[Relevant historical memories]".to_string()];
            for (i, item) in items.iter().enumerate() {
                let content_str = item
                    .value
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| item.value.to_string());
                lines.push(format!("{}. {}", i + 1, content_str));
            }
            lines.push("[The above memories are for reference, please answer with the current question in mind]".to_string());
            self.memory
                .context
                .lock()
                .await
                .push(Message::user(lines.join("\n")));
        }

        // Push multimodal user message
        self.memory.context.lock().await.push(message.clone());
        recalled
    }

    /// Save checkpoint (for chat mode)
    #[allow(dead_code)]
    pub(crate) async fn save_checkpoint(&self) {
        self.persist_runtime_state().await;
    }
}
