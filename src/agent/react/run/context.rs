//! 上下文管理 + 长期记忆 + 持久化 + 审计

use super::super::ReactAgent;
use super::stream_loop::StreamMode;
use crate::llm::types::Message;
use crate::memory::checkpointer::ThreadState;
use crate::memory::conversation::{NewConversation, project_messages};
use crate::memory::store::SearchQuery;
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

    /// 根据快照策略自动捕获状态快照
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
                "📸 自动快照已捕获"
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

    /// 重置消息历史，仅保留 system prompt，确保每次执行互不干扰
    pub(crate) async fn reset_messages(&self) {
        let mut ctx = self.memory.context.lock().await;
        ctx.clear();
        ctx.push(Message::system(self.config.system_prompt.clone()));
    }

    pub(crate) async fn restore_thread_context(&self) {
        let agent = self.config.agent_name.clone();
        if let (Some(cp), Some(tid)) = (&self.memory.checkpointer, &self.config.session_id) {
            match cp.get_state(tid).await {
                Ok(Some(state)) => {
                    info!(agent = %agent, session_id = %tid, "🔄 从线程状态恢复会话");
                    self.memory
                        .context
                        .lock()
                        .await
                        .set_messages(state.messages);
                }
                Ok(None) => {
                    debug!(agent = %agent, session_id = %tid, "新会话，从空上下文开始");
                    self.reset_messages().await;
                }
                Err(e) => {
                    warn!(agent = %agent, error = %e, "⚠️ 线程状态加载失败，从空上下文开始");
                    self.reset_messages().await;
                }
            }
        } else {
            self.reset_messages().await;
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
                "⚠️ 已配置 ConversationStore，但缺少 conversation_id，跳过历史投影"
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
                "⚠️ 对话历史投影保存失败"
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
                    debug!(agent = %self.config.agent_name, session_id = %tid, checkpoint_id = %cid, "🔖 线程状态已保存")
                }
                Err(e) => {
                    warn!(agent = %self.config.agent_name, error = %e, "⚠️ 线程状态保存失败")
                }
            }
        }
        self.sync_conversation_projection().await;
    }

    pub(crate) async fn inject_hook_messages(
        &self,
        tool_name: &str,
        phase: &str,
        messages: &[String],
    ) {
        let mut ctx = self.memory.context.lock().await;
        for message in messages {
            ctx.push(Message::system(format!(
                "[Skill Hook:{phase}:{tool_name}]\n{message}"
            )));
        }
    }

    pub(crate) async fn apply_hook_messages(
        &self,
        tool_name: &str,
        hook_messages: &HookMessageBatches,
    ) {
        self.inject_hook_messages(tool_name, "pre", &hook_messages.pre)
            .await;
        self.inject_hook_messages(tool_name, "post", &hook_messages.post)
            .await;
    }

    /// 流式执行的公共初始化逻辑
    ///
    /// 根据模式决定是否重置上下文、是否从 checkpoint 恢复。
    /// 返回召回的长期记忆数量（0 表示无记忆注入）。
    pub(crate) async fn prepare_stream_context(&self, mode: StreamMode, input: &str) -> usize {
        match mode {
            StreamMode::Execute => {
                self.restore_thread_context().await;
            }
            StreamMode::Chat => {
                // 多轮对话模式：不重置上下文
            }
        }

        // 注入相关长期记忆
        let mut recalled = 0usize;
        if let Ok(items) = self.recall_long_term_memories(input).await
            && !items.is_empty()
        {
            recalled = items.len();
            let mut lines = vec!["[相关历史记忆]".to_string()];
            for (i, item) in items.iter().enumerate() {
                let content_str = item
                    .value
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| item.value.to_string());
                lines.push(format!("{}. {}", i + 1, content_str));
            }
            lines.push("[以上记忆供参考，请结合当前问题作答]".to_string());
            self.memory
                .context
                .lock()
                .await
                .push(Message::user(lines.join("\n")));
        }

        // 推送用户消息
        self.memory
            .context
            .lock()
            .await
            .push(Message::user(input.to_string()));
        recalled
    }

    /// 流式执行上下文初始化（多模态消息版本）
    ///
    /// 与 `prepare_stream_context` 相同，但接受预构建的 `Message` 而不是字符串，
    /// 支持多模态 content parts（图片、文件等）。
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

        // 从消息中提取文本用于长期记忆检索
        let text = message.content.as_text().unwrap_or_default();
        let mut recalled = 0usize;
        if !text.is_empty()
            && let Ok(items) = self.recall_long_term_memories(&text).await
            && !items.is_empty()
        {
            recalled = items.len();
            let mut lines = vec!["[相关历史记忆]".to_string()];
            for (i, item) in items.iter().enumerate() {
                let content_str = item
                    .value
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| item.value.to_string());
                lines.push(format!("{}. {}", i + 1, content_str));
            }
            lines.push("[以上记忆供参考，请结合当前问题作答]".to_string());
            self.memory
                .context
                .lock()
                .await
                .push(Message::user(lines.join("\n")));
        }

        // 推送多模态用户消息
        self.memory.context.lock().await.push(message.clone());
        recalled
    }

    /// 保存 checkpoint（用于 chat 模式）
    pub(crate) async fn save_checkpoint(&self) {
        self.persist_runtime_state().await;
    }
}
