//! Context management + long-term memory + persistence + audit

use super::super::ReactAgent;
use super::types::StreamMode;
use crate::llm::types::{Message, Role};
use crate::memory::SearchQuery;
use crate::skills::hooks::{HookContext, HookEvent};
use echo_core::memory::types::MemoryStatus;
use echo_state::memory::typed_store::{TypedMemoryEntry, TypedMemoryStore};
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

    /// (stage4 A2/C1) Unified recall over the single `["agent","memories"]`
    /// namespace with composite-score re-ranking. Replaces the old multi-
    /// namespace reads (legacy + typed_memories + l3_promoted) and the
    /// score-only sort. Superseded memories are filtered out (割裂点5).
    pub(crate) async fn recall_long_term_memories(
        &self,
        query: &str,
    ) -> crate::error::Result<Vec<crate::memory::store::StoreItem>> {
        let Some(store) = &self.memory.store else {
            return Ok(vec![]);
        };
        let ns = crate::evolution::layer::WARM_NAMESPACE; // ["agent","memories"]
        let top_k = 5;

        // 1. Candidates via hybrid search; any hybrid error falls back to keyword
        //    search (no string-matching on error text).
        let candidates = match store
            .search_with(ns, SearchQuery::hybrid(query, top_k * 3))
            .await
        {
            Ok(items) => items,
            Err(_) => store.search(ns, query, top_k * 3).await?,
        };

        // 2. Composite-score re-rank + status filter (Superseded dropped).
        let mut scored: Vec<(f64, TypedMemoryEntry)> = candidates
            .into_iter()
            .filter_map(|item| {
                let entry = TypedMemoryEntry::from_store_item(item);
                if entry.meta.status == MemoryStatus::Superseded {
                    return None;
                }
                let sim = entry.raw.score.unwrap_or(0.0) as f64;
                let age = age_days_from_storeitem(&entry.raw);
                let s = composite_score(sim, age, entry.meta.recall_weight as f64);
                Some((s, entry))
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        // 3. recall_count +1 (fire-and-forget; Dreaming consumes it in stage 2).
        let typed_for_count = TypedMemoryStore::new(store.clone());
        let keys: Vec<String> = scored.iter().map(|(_, e)| e.raw.key.clone()).collect();
        tokio::spawn(async move {
            for key in keys {
                let _ = incr_recall_count(&typed_for_count, &["agent", "memories"], &key).await;
            }
        });

        Ok(scored.into_iter().map(|(_, e)| e.raw).collect())
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
            ctx.push(runtime_context_note(
                &format!("{source}:{phase}:{identifier}"),
                message,
            ));
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
            match self.activate_skill_for_context(skill).await {
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
    pub(crate) async fn prepare_stream_context(&self, mode: StreamMode, input: &str) -> usize {
        // Clear read-before-edit tracking for the new conversation turn
        // (converged with prepare_react_context; the entry layer no longer
        // clears it separately to avoid a double clear).
        self.clear_read_files();
        match mode {
            StreamMode::Execute => {
                self.restore_thread_context().await;
            }
            StreamMode::Chat => {
                // Multi-turn chat mode: do not reset context
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
        context.push(Message::user(input.to_string()));
        if let Some(runtime_context) =
            format_turn_runtime_context(memory_context.as_deref(), ws_block.as_str())
        {
            context.push(runtime_context_note("turn", &runtime_context));
        }
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
        // Clear read-before-edit tracking (see prepare_stream_context).
        self.clear_read_files();
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
        context.push(message.clone());
        if let Some(runtime_context) =
            format_turn_runtime_context(memory_context.as_deref(), ws_block.as_str())
        {
            context.push(runtime_context_note("turn", &runtime_context));
        }
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

        recalled
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
    // (stage4 G1) Wrap in <protected_memory> so compression's protected_markers
    // keeps recalled memories from being evicted (割裂点7 折中 fix; stage 2
    // pre_compaction_flush is the thorough fix).
    format!(
        "<protected_memory>\n{}\n</protected_memory>",
        lines.join("\n")
    )
}

/// (stage4 C1) Composite recall score:
/// `S = 0.5·sim + 0.3·decay(age, 30d) + 0.2·recall_weight`.
fn composite_score(sim: f64, age_days: f64, recall_weight: f64) -> f64 {
    0.5 * sim + 0.3 * 0.5_f64.powf(age_days / 30.0) + 0.2 * recall_weight
}

/// (stage4 C1) Age in days from `StoreItem::created_at` (Unix seconds).
fn age_days_from_storeitem(item: &crate::memory::store::StoreItem) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(item.created_at) as f64 / 86400.0
}

/// (stage4 C1) Increment `recall_count` for a recalled memory. `update_meta`
/// takes a full `MemoryMeta` (not a closure), so get-modify-put. Fire-and-
/// forget from recall; lost increments are acceptable (diagnostic counter).
async fn incr_recall_count(
    typed: &TypedMemoryStore,
    ns: &[&str],
    key: &str,
) -> crate::error::Result<()> {
    if let Some(entry) = typed.get_typed(ns, key).await? {
        let mut meta = entry.meta;
        meta.recall_count = meta.recall_count.saturating_add(1);
        typed.update_meta(ns, key, meta).await?;
    }
    Ok(())
}

pub(crate) fn format_turn_runtime_context(
    memory_context: Option<&str>,
    workspace_context: &str,
) -> Option<String> {
    let mut blocks = Vec::new();
    if !workspace_context.trim().is_empty() {
        blocks.push(workspace_context.trim().to_string());
    }
    if let Some(memory_context) = memory_context
        && !memory_context.trim().is_empty()
    {
        blocks.push(memory_context.trim().to_string());
    }
    if blocks.is_empty() {
        None
    } else {
        Some(blocks.join("\n\n"))
    }
}
