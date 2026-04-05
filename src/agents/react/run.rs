//! ReactAgent 执行引擎
//!
//! 包含 ReAct 循环的所有内部实现：
//! - `reset_messages` / `execute_tool` / `execute_tool_feedback`
//! - `think`（LLM 推理）
//! - `process_steps`（工具并发调度）
//! - `run_direct` / `run_chat_direct` / `run_react_loop`（ReAct 主循环）
//! - `run_stream_loop`（流式执行公共逻辑）

use super::{ReactAgent, StepType, TOOL_FINAL_ANSWER, is_retryable_llm_error};
use crate::agent::AgentEvent;
use crate::error::{AgentError, ReactError, Result, ToolError};
use crate::guard::GuardDirection;
#[cfg(feature = "human-loop")]
use crate::human_loop::{HumanLoopRequest, HumanLoopResponse};
use crate::llm::types::{FunctionCall, Message, ToolCall as LlmToolCall};
use crate::llm::{chat, stream_chat};
use crate::tools::ToolParameters;
use futures::StreamExt;
use futures::future::join_all;
use futures::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;
use tracing::{Instrument, debug, info, info_span, warn};

// ── 流式执行模式 ─────────────────────────────────────────────────────────────

/// 流式执行的模式配置
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamMode {
    /// 单轮执行模式：重置上下文，从 checkpoint 恢复
    Execute,
    /// 多轮对话模式：保留上下文，不重置
    Chat,
}

impl ReactAgent {
    #[cfg(feature = "human-loop")]
    #[allow(deprecated)]
    fn get_approval_manager(
        &self,
    ) -> Result<std::sync::RwLockReadGuard<'_, crate::human_loop::HumanApprovalManager>> {
        self.human_in_loop.read().map_err(|e| {
            tracing::error!("Human approval system lock poisoned: {}", e);
            ReactError::Agent(AgentError::InitializationFailed(
                "Human approval system unavailable due to internal error".to_string(),
            ))
        })
    }

    /// 判断工具是否需要人工审批（供 process_steps 决定串行/并行执行）
    ///
    /// 优先使用 PermissionService，回退到旧的两阶段检查。
    #[cfg(feature = "human-loop")]
    async fn tool_needs_approval(&self, tool_name: &str) -> bool {
        // 1. PermissionService 统一管线（快速路径：不触发 handler）
        if let Some(service) = &self.permission_service {
            let mode = service.mode().await;
            // BypassPermissions / Auto / DontAsk 模式不需要串行等待审批
            if matches!(
                mode,
                crate::tools::permission::PermissionMode::BypassPermissions
                    | crate::tools::permission::PermissionMode::DontAsk
            ) {
                return false;
            }
            // Plan 模式下写操作直接拒绝（不需要串行审批）
            if mode == crate::tools::permission::PermissionMode::Plan {
                return false;
            }

            // 检查规则注册表：如果没有匹配规则，默认需要审批
            let decision = service
                .check(tool_name, &serde_json::json!({}))
                .await
                .unwrap_or(crate::tools::permission::PermissionDecision::RequireApproval);

            return decision.requires_approval();
        }

        // 2. PermissionPolicy 回退
        if let Some(policy) = &self.permission_policy {
            let tool_perms = self
                .tool_manager
                .get_tool(tool_name)
                .map(|t| t.permissions())
                .unwrap_or_default();

            if !tool_perms.is_empty() {
                let decision = policy.check(tool_name, &tool_perms).await;
                if matches!(
                    decision,
                    crate::tools::permission::PermissionDecision::RequireApproval
                        | crate::tools::permission::PermissionDecision::Ask { .. }
                ) {
                    return true;
                }
            }
        }

        // 3. HumanApprovalManager 回退（向后兼容）
        #[allow(deprecated)]
        if let Ok(manager) = self.get_approval_manager() {
            if manager.needs_approval(tool_name) {
                return true;
            }
        }

        false
    }

    #[cfg(not(feature = "human-loop"))]
    async fn tool_needs_approval(&self, _tool_name: &str) -> bool {
        false
    }

    /// 统一审批检查入口
    ///
    /// 优先使用 PermissionService（统一管线: mode → rules → cache → denial → classifier/handler），
    /// 回退到旧的 PermissionPolicy + HumanApprovalManager 两阶段检查。
    ///
    /// 返回 `Ok(Some(modified_args))` 表示用户在审批时修改了参数，调用方应使用修改后的参数。
    /// 返回 `Ok(None)` 表示审批通过，使用原始参数。
    #[cfg(feature = "human-loop")]
    async fn check_tool_approval(&self, tool_name: &str, input: &Value) -> Result<Option<Value>> {
        let agent = &self.config.agent_name;

        // ── Phase 0: PermissionService 统一管线 ──
        if let Some(service) = &self.permission_service {
            let tool_perms = self
                .tool_manager
                .get_tool(tool_name)
                .map(|t| t.permissions())
                .unwrap_or_default();

            let decision = service
                .check_with_permissions(tool_name, input, &tool_perms)
                .await
                .map_err(|e| ReactError::Other(format!("PermissionService 错误: {e}")))?;

            match decision {
                crate::tools::permission::PermissionDecision::Allow => {
                    let modified = service.take_modified_args().await;
                    return Ok(modified);
                }
                crate::tools::permission::PermissionDecision::Deny { reason } => {
                    self.log_permission_denied(tool_name, &tool_perms, &reason)
                        .await;
                    return Err(
                        ReactError::Other(format!("工具 {tool_name} 权限不足: {reason}")).into(),
                    );
                }
                crate::tools::permission::PermissionDecision::RequireApproval => {
                    info!(agent = %agent, tool = %tool_name, "🔐 权限服务要求人工审批");
                    return self.request_human_approval(tool_name, input).await;
                }
                crate::tools::permission::PermissionDecision::Ask { suggestions } => {
                    return self.handle_ask_decision(tool_name, &suggestions).await;
                }
            }
        }

        // ── Phase 1 (回退): PermissionPolicy 检查 ──
        if let Some(policy) = &self.permission_policy {
            let tool_perms = self
                .tool_manager
                .get_tool(tool_name)
                .map(|t| t.permissions())
                .unwrap_or_default();

            if !tool_perms.is_empty() {
                let decision = policy.check(tool_name, &tool_perms).await;
                match decision {
                    crate::tools::permission::PermissionDecision::Allow => {}
                    crate::tools::permission::PermissionDecision::Deny { reason } => {
                        self.log_permission_denied(tool_name, &tool_perms, &reason).await;
                        return Err(
                            ReactError::Other(format!("工具 {tool_name} 权限不足: {reason}"))
                                .into(),
                        );
                    }
                    crate::tools::permission::PermissionDecision::RequireApproval => {
                        info!(agent = %agent, tool = %tool_name, "🔐 权限策略要求人工审批");
                        return self.request_human_approval(tool_name, input).await;
                    }
                    crate::tools::permission::PermissionDecision::Ask { suggestions } => {
                        return self
                            .handle_ask_decision(tool_name, &suggestions)
                            .await;
                    }
                }
            }
        }

        // ── Phase 2 (回退): HumanApprovalManager 检查（向后兼容）──
        {
            let needs_approval = {
                let approval_manager = self.get_approval_manager()?;
                approval_manager.needs_approval(tool_name)
            };

            if needs_approval {
                warn!(agent = %agent, tool = %tool_name, "⚠️ 工具被标记为需要人工审批");
                return self.request_human_approval(tool_name, input).await;
            }
        }

        Ok(None)
    }

    /// 记录权限拒绝审计日志
    #[cfg(feature = "human-loop")]
    async fn log_permission_denied(
        &self,
        tool_name: &str,
        tool_perms: &[crate::tools::permission::ToolPermission],
        reason: &str,
    ) {
        let agent = &self.config.agent_name;
        warn!(agent = %agent, tool = %tool_name, reason = %reason, "🔒 权限拒绝");
        if let Some(al) = &self.audit_logger {
            let event = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                agent.to_string(),
                crate::audit::AuditEventType::PermissionDenied {
                    tool: tool_name.to_string(),
                    required: tool_perms.to_vec(),
                    reason: reason.to_string(),
                },
            );
            let _ = al.log(event).await;
        }
    }

    /// 处理 Ask 决策 — 向用户确认工具执行
    #[cfg(feature = "human-loop")]
    async fn handle_ask_decision(
        &self,
        tool_name: &str,
        suggestions: &[String],
    ) -> Result<Option<Value>> {
        let agent = &self.config.agent_name;
        info!(agent = %agent, tool = %tool_name, "❓ 权限需要用户确认");
        let prompt = format!(
            "工具 '{}' 需要确认。建议选项：{}",
            tool_name,
            suggestions.join(", ")
        );
        let req = HumanLoopRequest::input(prompt);
        match self.approval_provider.request(req).await? {
            HumanLoopResponse::Text(response) => {
                if response.to_lowercase().contains("拒绝")
                    || response.to_lowercase().contains("deny")
                {
                    return Err(ReactError::Other(format!(
                        "工具 {tool_name} 用户选择拒绝：{response}"
                    ))
                    .into());
                }
                info!(agent = %agent, tool = %tool_name, "✅ 用户确认执行");
            }
            HumanLoopResponse::Approved => {
                info!(agent = %agent, tool = %tool_name, "✅ 用户确认执行");
            }
            HumanLoopResponse::Rejected { reason } => {
                return Err(ReactError::Other(format!(
                    "工具 {tool_name} 用户拒绝{}",
                    reason.map(|r| format!("，原因：{r}")).unwrap_or_default()
                ))
                .into());
            }
            _ => {
                return Err(
                    ReactError::Other(format!("工具 {tool_name} 用户确认超时")).into(),
                );
            }
        }
        Ok(None)
    }

    #[cfg(not(feature = "human-loop"))]
    async fn check_tool_approval(&self, _tool_name: &str, _input: &Value) -> Result<Option<Value>> {
        Ok(None)
    }

    /// 请求人工审批并记录审计日志
    ///
    /// 处理所有 `HumanLoopResponse` 变体，统一记录审批请求/完成审计事件。
    /// 风险等级根据工具权限动态计算，而非硬编码。
    ///
    /// 返回 `Ok(Some(modified_args))` 表示用户修改了参数后批准。
    /// 返回 `Ok(None)` 表示用户直接批准。
    #[cfg(feature = "human-loop")]
    async fn request_human_approval(&self, tool_name: &str, input: &Value) -> Result<Option<Value>> {
        let agent = &self.config.agent_name;
        let approval_start = std::time::Instant::now();

        // 根据工具权限动态计算风险等级
        let tool_perms = self
            .tool_manager
            .get_tool(tool_name)
            .map(|t| t.permissions())
            .unwrap_or_default();
        let risk_level = crate::human_loop::RiskLevel::from_permissions(&tool_perms);
        let risk_level_str = format!("{:?}", risk_level).to_lowercase();

        // 审计：审批请求
        if let Some(al) = &self.audit_logger {
            let args_hash = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                format!("{input}").hash(&mut hasher);
                format!("{:016x}", hasher.finish())
            };
            let event = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                agent.clone(),
                crate::audit::AuditEventType::ApprovalRequested {
                    tool: tool_name.to_string(),
                    args_hash,
                    risk_level: risk_level_str,
                },
            );
            let _ = al.log(event).await;
        }

        let req = HumanLoopRequest::approval(tool_name, input.clone());
        let response = self.approval_provider.request(req).await?;

        // 审计：审批完成
        if let Some(al) = &self.audit_logger {
            let duration_ms = approval_start.elapsed().as_millis() as u64;
            let (decision, scope, reason) = match &response {
                HumanLoopResponse::Approved => ("approved".into(), "once".into(), None),
                HumanLoopResponse::ApprovedWithScope { scope: s } => {
                    ("approved".into(), format!("{s:?}"), None)
                }
                HumanLoopResponse::ModifiedArgs { args: _, scope: s } => {
                    ("modified".into(), format!("{s:?}"), None)
                }
                HumanLoopResponse::Rejected { reason: r } => {
                    ("rejected".into(), "once".into(), r.clone())
                }
                HumanLoopResponse::Timeout => ("timeout".into(), "once".into(), None),
                HumanLoopResponse::Deferred => ("deferred".into(), "once".into(), None),
                HumanLoopResponse::Text(_) => ("unexpected".into(), "once".into(), None),
            };
            let event = crate::audit::AuditEvent::now(
                self.config.session_id.clone(),
                agent.clone(),
                crate::audit::AuditEventType::ApprovalCompleted {
                    tool: tool_name.to_string(),
                    decision,
                    scope,
                    reason,
                    duration_ms,
                },
            );
            let _ = al.log(event).await;
        }

        match response {
            HumanLoopResponse::Approved => {
                info!(agent = %agent, tool = %tool_name, "✅ 用户批准执行工具");
                Ok(None)
            }
            HumanLoopResponse::ApprovedWithScope { scope: _ } => {
                info!(agent = %agent, tool = %tool_name, "✅ 用户带范围批准执行工具");
                Ok(None)
            }
            HumanLoopResponse::ModifiedArgs { args, scope: _ } => {
                info!(agent = %agent, tool = %tool_name, "✅ 用户修改参数后批准执行工具");
                Ok(Some(args))
            }
            HumanLoopResponse::Rejected { reason } => {
                warn!(agent = %agent, tool = %tool_name, "❌ 用户拒绝执行工具");
                Err(ReactError::Other(format!(
                    "用户已拒绝执行工具 {}{}",
                    tool_name,
                    reason.map(|r| format!("，原因：{r}")).unwrap_or_default()
                ))
                .into())
            }
            HumanLoopResponse::Timeout => {
                warn!(agent = %agent, tool = %tool_name, "⏰ 审批超时");
                Err(ReactError::Other(format!("工具 {tool_name} 审批超时，已跳过执行")).into())
            }
            HumanLoopResponse::Deferred => {
                warn!(agent = %agent, tool = %tool_name, "⏸️ 用户推迟审批");
                Err(ReactError::Other(format!("工具 {tool_name} 审批被推迟，已跳过执行")).into())
            }
            HumanLoopResponse::Text(_) => {
                warn!(agent = %agent, tool = %tool_name, "⚠️ 审批请求收到意外的 Text 响应");
                Err(ReactError::Other(format!("工具 {tool_name} 审批异常，已跳过执行")).into())
            }
        }
    }

    /// 根据快照策略自动捕获状态快照
    pub(crate) fn auto_snapshot(&mut self, iteration: usize) {
        let should = self
            .snapshot_manager
            .as_ref()
            .is_some_and(|mgr| mgr.should_capture(iteration));

        if should {
            let messages = self.context.messages();
            let id = self
                .snapshot_manager
                .as_mut()
                .unwrap()
                .capture(iteration, messages);
            debug!(
                agent = %self.config.agent_name,
                iteration = iteration,
                snapshot_id = %id,
                "📸 自动快照已捕获"
            );
        }
    }

    /// 重置消息历史，仅保留 system prompt，确保每次执行互不干扰
    pub(crate) fn reset_messages(&mut self) {
        self.context.clear();
        self.context
            .push(Message::system(self.config.system_prompt.clone()));
    }

    /// 执行工具，保留工具返回的真实错误信息
    pub(crate) async fn execute_tool(&self, tool_name: &str, input: &Value) -> Result<String> {
        let agent = &self.config.agent_name;
        let callbacks = self.config.callbacks.clone();
        let params: ToolParameters = if let Value::Object(map) = input {
            map.clone().into_iter().collect()
        } else {
            HashMap::new()
        };

        for cb in &callbacks {
            cb.on_tool_start(agent, tool_name, input).await;
        }

        info!(agent = %agent, tool = %tool_name, "🔧 开始执行工具");
        debug!(agent = %agent, tool = %tool_name, params = %input, "工具参数详情");

        // ── PreToolUse hooks（审批前执行，允许 hook 拦截或修改参数）──
        let mut effective_params = params;
        let mut hook_modified_input = input.clone();
        {
            let hook_reg = self.hook_registry.read().await;
            if !hook_reg.is_empty() {
                let hook_result = hook_reg.run_pre_tool_use(tool_name, input).await;

                if hook_result.block {
                    let reason = hook_result
                        .block_reason
                        .unwrap_or_else(|| "blocked by skill hook".into());
                    info!(agent = %agent, tool = %tool_name, reason = %reason, "Hook blocked tool");
                    return Ok(format!("Tool {} blocked by hook: {}", tool_name, reason));
                }

                if let Some(updated) = hook_result.updated_input {
                    hook_modified_input = updated.clone();
                    if let Value::Object(map) = &updated {
                        effective_params =
                            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    }
                }
            }
        }

        // ── 统一审批检查 ──
        // PermissionService → PermissionPolicy → HumanApprovalManager（向后兼容）
        // 返回用户在审批时修改的参数（如有）
        let approval_modified_args = self
            .check_tool_approval(tool_name, &hook_modified_input)
            .await?;

        // 如果用户在审批时修改了参数，覆盖工具的实际执行参数
        if let Some(modified) = approval_modified_args {
            if let Value::Object(map) = &modified {
                effective_params = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            }
        }

        let result = self
            .tool_manager
            .execute_tool(tool_name, effective_params)
            .await?;

        // ── PostToolUse hooks ──
        {
            let hook_reg = self.hook_registry.read().await;
            if !hook_reg.is_empty() {
                let _post_result = hook_reg
                    .run_post_tool_use(tool_name, input, &result.output)
                    .await;
            }
        }

        if result.success {
            info!(agent = %agent, tool = %tool_name, "📤 工具执行成功");
            debug!(agent = %agent, tool = %tool_name, output = %result.output, "工具返回详情");
            for cb in callbacks.iter() {
                cb.on_tool_end(agent, tool_name, &result.output).await;
            }
            Ok(result.output)
        } else {
            let error_msg = result
                .error
                .clone()
                .unwrap_or_else(|| result.output.clone());
            warn!(agent = %agent, tool = %tool_name, error = %error_msg, "💥 工具执行失败");
            let err = ReactError::from(ToolError::ExecutionFailed {
                tool: tool_name.to_string(),
                message: error_msg,
            });
            for cb in &callbacks {
                cb.on_tool_error(agent, tool_name, &err).await;
            }
            Err(err)
        }
    }

    /// 对工具输出进行 token 预算截断。
    ///
    /// 当 `max_tool_output_tokens` 已配置且输出估算 token 超限时，
    /// 截断文本并在尾部追加 `[输出已截断，共 N tokens]` 提示。
    pub(crate) fn truncate_tool_output(&self, output: String) -> String {
        let Some(max_tokens) = self.config.max_tool_output_tokens else {
            return output;
        };
        let tokenizer = self.context.tokenizer();
        let token_count = tokenizer.count_tokens(&output);
        if token_count <= max_tokens {
            return output;
        }

        // 按字符比例估算截断位置
        let ratio = max_tokens as f64 / token_count as f64;
        let char_limit = (output.len() as f64 * ratio * 0.95) as usize;
        let truncated: String = output.chars().take(char_limit).collect();
        let suffix = format!(
            "\n[输出已截断，共 {} tokens，保留前 {} tokens]",
            token_count, max_tokens
        );
        format!("{truncated}{suffix}")
    }

    /// 执行工具，并根据 `tool_error_feedback` 配置决定失败时的行为：
    /// - `true`（默认）：将错误信息转换为工具观测值回传给 LLM，让模型自行纠错
    /// - `false`：直接向上抛出 `Err`，与旧行为一致
    ///
    /// `final_answer` 工具始终保持原始错误语义，不会被软化。
    /// 工具输出会经过 `truncate_tool_output` 进行 token 预算截断。
    pub(crate) async fn execute_tool_feedback(
        &self,
        tool_name: &str,
        input: &Value,
    ) -> Result<String> {
        match self.execute_tool(tool_name, input).await {
            Ok(result) => Ok(self.truncate_tool_output(result)),
            Err(e) if self.config.tool_error_feedback && tool_name != TOOL_FINAL_ANSWER => {
                warn!(
                    agent = %self.config.agent_name,
                    tool = %tool_name,
                    error = %e,
                    "⚠️ 工具错误已转为观测值回传 LLM"
                );
                Ok(format!(
                    "[工具执行失败] {e}\n提示：请根据错误信息调整参数后重试，或换用其他工具。"
                ))
            }
            Err(e) => Err(e),
        }
    }

    /// 调用 LLM 推理，返回本轮的步骤列表。
    ///
    /// 每次调用前先通过 `ContextManager::prepare` 自动压缩超限的历史消息，
    /// 再将压缩后的消息列表传给 LLM；LLM 的响应追加回 context。
    pub(crate) async fn think(&mut self) -> Result<Vec<StepType>> {
        let agent = self.config.agent_name.clone();
        let callbacks = self.config.callbacks.clone();
        let mut res = Vec::new();

        debug!(agent = %agent, model = %self.config.model_name, "🧠 LLM 思考中...");

        // 预检查：当可用 token 余量不足时，主动触发压缩
        if self.config.token_limit != usize::MAX {
            let current_tokens = self.context.token_estimate();
            let threshold = (self.config.token_limit as f64
                * (1.0 - self.config.compress_threshold_ratio))
                as usize;
            if current_tokens > threshold && self.context.has_compressor() {
                debug!(
                    agent = %agent,
                    current = current_tokens,
                    threshold = threshold,
                    limit = self.config.token_limit,
                    "📦 Token 余量不足，主动触发上下文压缩"
                );
            }
        }

        let messages = self.context.prepare(None).await?;

        for cb in &callbacks {
            cb.on_think_start(&agent, &messages).await;
        }

        let tools = self.tool_manager.get_openai_tools();
        let max_retries = self.config.llm_max_retries;
        let retry_delay = self.config.llm_retry_delay_ms;
        // 在循环外克隆一次，避免重复克隆
        let client = self.client.clone();
        let model_name = self.config.model_name.clone();
        let response_format = self.config.response_format.clone();

        let mut response_result: Result<_> = Err(ReactError::Agent(AgentError::NoResponse));
        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay_ms = retry_delay * (1u64 << (attempt - 1).min(5));
                warn!(
                    agent = %agent,
                    attempt = attempt,
                    max = max_retries,
                    delay_ms = delay_ms,
                    "⚠️ LLM 请求失败，{delay_ms}ms 后重试（{attempt}/{max_retries}）"
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            }
            response_result = chat(
                client.clone(),
                model_name.as_str(),
                messages.clone(),
                Some(0.7),
                Some(8192u32),
                Some(false),
                Some(tools.clone()),
                None,
                response_format.clone(),
            )
            .await;
            match &response_result {
                Ok(_) => {
                    if attempt > 0 {
                        info!(agent = %agent, attempt, "✅ LLM 重试成功");
                    }
                    break;
                }
                Err(e) if attempt < max_retries && is_retryable_llm_error(e) => {
                    warn!(agent = %agent, error = %e, "LLM 可重试错误");
                }
                Err(_) => break,
            }
        }

        let message = response_result?
            .choices
            .first()
            .ok_or(ReactError::Agent(AgentError::NoResponse))?
            .message
            .clone();

        if let Some(tool_calls) = &message.tool_calls {
            self.context.push(message.clone());
            let tool_names: Vec<&str> = tool_calls
                .iter()
                .map(|c| c.function.name.as_str())
                .collect();
            info!(
                agent = %agent,
                tools = ?tool_names,
                "🧠 LLM 决定调用 {} 个工具",
                tool_calls.len()
            );
            for call in tool_calls {
                res.push(StepType::Call {
                    tool_call_id: call.id.clone(),
                    function_name: call.function.name.clone(),
                    arguments: serde_json::from_str(&call.function.arguments)?,
                });
            }
        } else if let Some(content) = &message.content {
            self.context.push(message.clone());
            debug!(agent = %agent, "🧠 LLM 返回文本响应");
            res.push(StepType::Thought(content.to_string()));
        }

        for cb in &callbacks {
            cb.on_think_end(&agent, &res).await;
        }

        Ok(res)
    }

    /// 处理一轮思考产生的步骤：
    /// - 有工具调用 → 并行执行（需要审批的工具强制串行），`final_answer` 时返回答案
    /// - 无工具调用 → 纯文本响应视为最终答案，直接返回
    pub(crate) async fn process_steps(&mut self, steps: Vec<StepType>) -> Result<Option<String>> {
        let agent = self.config.agent_name.clone();
        let mut tool_calls = Vec::new();
        let mut last_thought: Option<String> = None;

        for step in steps {
            match step {
                StepType::Call {
                    tool_call_id,
                    function_name,
                    arguments,
                } => {
                    tool_calls.push((tool_call_id, function_name, arguments));
                }
                StepType::Thought(content) => {
                    debug!(agent = %agent, "🤔 思考: {}", content);
                    last_thought = Some(content);
                }
            }
        }

        if tool_calls.is_empty() {
            return Ok(last_thought.filter(|s| !s.is_empty()));
        }

        if tool_calls.len() > 1 {
            let tool_names: Vec<&str> = tool_calls.iter().map(|(_, n, _)| n.as_str()).collect();
            let max_concurrency = self.tool_manager.max_concurrency();
            info!(
                agent = %agent,
                tools = ?tool_names,
                max_concurrency = ?max_concurrency,
                "⚡ 并发执行 {} 个工具调用",
                tool_calls.len()
            );
        }

        #[cfg(feature = "human-loop")]
        let has_approval_tools = {
            let mut result = false;
            for (_, name, _) in &tool_calls {
                if self.tool_needs_approval(name).await {
                    result = true;
                    break;
                }
            }
            result
        };
        #[cfg(not(feature = "human-loop"))]
        let has_approval_tools = false;

        if has_approval_tools {
            info!(agent = %agent, "⚠️ 检测到需人工审批工具，切换为串行执行");
            for (tool_call_id, function_name, arguments) in tool_calls {
                let result = self
                    .execute_tool_feedback(&function_name, &arguments)
                    .await?;
                self.context.push(Message::tool_result(
                    tool_call_id,
                    function_name.clone(),
                    result.clone(),
                ));
                if function_name == TOOL_FINAL_ANSWER {
                    info!(agent = %agent, "🏁 最终答案已生成");
                    return Ok(Some(result));
                }
            }
        } else {
            let futures: Vec<_> = tool_calls
                .iter()
                .map(|(_, name, args)| {
                    self.execute_tool_feedback(name, args)
                        .instrument(info_span!("tool_execute", tool.name = %name))
                })
                .collect();
            let results = join_all(futures).await;

            let mut final_answer: Option<String> = None;
            for ((tool_call_id, function_name, _), result) in tool_calls.into_iter().zip(results) {
                let result = result?;
                self.context.push(Message::tool_result(
                    tool_call_id,
                    function_name.clone(),
                    result.clone(),
                ));
                if function_name == TOOL_FINAL_ANSWER {
                    info!(agent = %agent, "🏁 最终答案已生成");
                    final_answer = Some(result);
                }
            }
            if final_answer.is_some() {
                return Ok(final_answer);
            }
        }

        Ok(None)
    }

    /// 直接执行（无规划）：重置/恢复上下文，然后进入 ReAct 循环
    pub(crate) async fn run_direct(&mut self, task: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();

        if let (Some(cp), Some(tid)) = (&self.checkpointer, &self.config.session_id) {
            match cp.get(tid).await {
                Ok(Some(checkpoint)) => {
                    info!(agent = %agent, session_id = %tid, checkpoint_id = %checkpoint.checkpoint_id, "🔄 从 Checkpoint 恢复会话");
                    self.context.clear();
                    for msg in checkpoint.messages {
                        self.context.push(msg);
                    }
                }
                Ok(None) => {
                    debug!(agent = %agent, session_id = %tid, "新会话，从空上下文开始");
                    self.reset_messages();
                }
                Err(e) => {
                    warn!(agent = %agent, error = %e, "⚠️ Checkpoint 加载失败，从空上下文开始");
                    self.reset_messages();
                }
            }
        } else {
            self.reset_messages();
        }

        info!(agent = %agent, "🧠 Agent 开始执行任务");
        debug!(
            agent = %agent,
            task = %task,
            tools = ?self.tool_manager.list_tools(),
            max_iterations = self.config.max_iterations,
            "执行详情"
        );

        let model = self.config.model_name.clone();
        self.run_react_loop(task)
            .instrument(info_span!("agent_execute", agent.name = %agent, agent.model = %model))
            .await
    }

    /// 多轮对话：不重置上下文，直接追加消息后进入 ReAct 循环
    pub(crate) async fn run_chat_direct(&mut self, message: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();

        info!(agent = %agent, "💬 Agent 多轮对话中");
        debug!(
            agent = %agent,
            message = %message,
            tools = ?self.tool_manager.list_tools(),
            max_iterations = self.config.max_iterations,
            "对话详情"
        );

        let model = self.config.model_name.clone();
        self.run_react_loop(message)
            .instrument(info_span!("agent_chat", agent.name = %agent, agent.model = %model))
            .await
    }

    /// 核心 ReAct 循环（注入记忆 → 追加消息 → think/act 迭代）。
    /// `run_direct` 和 `run_chat_direct` 共享此实现。
    async fn run_react_loop(&mut self, message: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();
        let callbacks = self.config.callbacks.clone();

        // 输入护栏检查
        if let Some(gm) = &self.guard_manager {
            info!(agent = %agent, direction = "input", "🛡️ 护栏检查开始");
            let result = gm.check_all(message, GuardDirection::Input).await?;
            if let crate::guard::GuardResult::Block { reason } = &result {
                info!(agent = %agent, reason = %reason, "🛡️ 输入被护栏阻断");
                if let Some(al) = &self.audit_logger {
                    let event = crate::audit::AuditEvent::now(
                        self.config.session_id.clone(),
                        agent.clone(),
                        crate::audit::AuditEventType::GuardBlock {
                            guard: "guard_manager".to_string(),
                            direction: GuardDirection::Input,
                            reason: reason.clone(),
                        },
                    );
                    let _ = al.log(event).await;
                }
                return Ok(format!("请求被安全护栏拦截: {reason}"));
            }
        }

        if let Some(store) = &self.store {
            let agent_name = self.config.agent_name.clone();
            let ns = vec![agent_name.as_str(), "memories"];
            match store.semantic_search(&ns, message, 5).await {
                Ok(items) if !items.is_empty() => {
                    debug!(agent = %agent, count = items.len(), "📚 注入相关长期记忆");
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
                    self.context.push(Message::user(lines.join("\n")));
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(agent = %agent, error = %e, "⚠️ 长期记忆检索失败，跳过注入");
                }
            }
        }

        self.context.push(Message::user(message.to_string()));

        for iteration in 0..self.config.max_iterations {
            info!(agent = %agent, iteration = iteration + 1, "🔄 ReAct 迭代开始");

            for cb in &callbacks {
                cb.on_iteration(&agent, iteration).await;
            }

            debug!(agent = %agent, iteration = iteration + 1, "--- 迭代 ---");

            let think_model = self.config.model_name.clone();
            let steps = self
                .think()
                .instrument(info_span!("llm_think", model = %think_model))
                .await?;
            if steps.is_empty() {
                warn!(agent = %agent, "LLM 没有响应");
                return Err(ReactError::from(AgentError::NoResponse));
            }

            if let Some(mut answer) = self.process_steps(steps).await? {
                // 输出护栏检查
                if let Some(gm) = &self.guard_manager {
                    let result = gm.check_all(&answer, GuardDirection::Output).await?;
                    if let crate::guard::GuardResult::Block { reason } = &result {
                        info!(agent = %agent, reason = %reason, "🛡️ 输出被护栏阻断");
                        if let Some(al) = &self.audit_logger {
                            let event = crate::audit::AuditEvent::now(
                                self.config.session_id.clone(),
                                agent.clone(),
                                crate::audit::AuditEventType::GuardBlock {
                                    guard: "guard_manager".to_string(),
                                    direction: GuardDirection::Output,
                                    reason: reason.clone(),
                                },
                            );
                            let _ = al.log(event).await;
                        }
                        answer = format!("回复内容已被安全护栏过滤: {reason}");
                    }
                }

                // 最终快照
                self.auto_snapshot(iteration);

                for cb in &callbacks {
                    cb.on_final_answer(&agent, &answer).await;
                }
                info!(agent = %agent, "🏁 执行完毕");

                if let (Some(cp), Some(tid)) = (&self.checkpointer, self.config.session_id.clone())
                {
                    let messages = self.context.messages().to_vec();
                    match cp.put(&tid, messages).await {
                        Ok(cid) => {
                            debug!(agent = %agent, session_id = %tid, checkpoint_id = %cid, "🔖 Checkpoint 已保存")
                        }
                        Err(e) => {
                            warn!(agent = %agent, error = %e, "⚠️ Checkpoint 保存失败")
                        }
                    }
                }

                return Ok(answer);
            }

            // 迭代中间快照（尚未产生最终答案）
            self.auto_snapshot(iteration);
        }

        warn!(agent = %agent, max = self.config.max_iterations, "达到最大迭代次数");
        Err(ReactError::from(AgentError::MaxIterationsExceeded(
            self.config.max_iterations,
        )))
    }

    // ── 流式执行公共方法 ─────────────────────────────────────────────────────────

    /// 流式执行的公共初始化逻辑
    ///
    /// 根据模式决定是否重置上下文、是否从 checkpoint 恢复。
    /// 返回召回的长期记忆数量（0 表示无记忆注入）。
    pub(crate) async fn prepare_stream_context(&mut self, mode: StreamMode, input: &str) -> usize {
        match mode {
            StreamMode::Execute => {
                self.reset_messages();

                // 从 checkpoint 恢复（如果存在）
                if let (Some(cp), Some(tid)) = (&self.checkpointer, &self.config.session_id) {
                    match cp.get(tid).await {
                        Ok(Some(checkpoint)) => {
                            info!(
                                agent = %self.config.agent_name,
                                session_id = %tid,
                                "🔄 从 Checkpoint 恢复会话（流式）"
                            );
                            self.context.clear();
                            for msg in checkpoint.messages {
                                self.context.push(msg);
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!(
                                agent = %self.config.agent_name,
                                error = %e,
                                "⚠️ Checkpoint 加载失败"
                            );
                        }
                    }
                }
            }
            StreamMode::Chat => {
                // 多轮对话模式：不重置上下文
            }
        }

        // 注入相关长期记忆
        let mut recalled = 0usize;
        if let Some(store) = &self.store {
            let agent_name = self.config.agent_name.clone();
            let ns = vec![agent_name.as_str(), "memories"];
            if let Ok(items) = store.semantic_search(&ns, input, 5).await
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
                self.context.push(Message::user(lines.join("\n")));
            }
        }

        // 推送用户消息
        self.context.push(Message::user(input.to_string()));
        recalled
    }

    /// 流式执行的 LLM 请求（带重试）
    pub(crate) async fn create_llm_stream(
        &mut self,
        messages: Vec<Message>,
    ) -> Result<BoxStream<'static, Result<crate::llm::types::ChatCompletionChunk>>> {
        let agent = &self.config.agent_name;
        let tools_for_stream: Option<Vec<_>> = if self.config.enable_tool {
            let tools = self.tool_manager.get_openai_tools();
            if tools.is_empty() { None } else { Some(tools) }
        } else {
            None
        };

        let max_retries = self.config.llm_max_retries;
        let retry_delay = self.config.llm_retry_delay_ms;
        let client = self.client.clone();
        let model_name = self.config.model_name.clone();
        let response_format = self.config.response_format.clone();

        info!(agent = %agent, model = %model_name, "📡 创建 LLM 流式请求");

        let mut stream_result: Result<_> = Err(ReactError::Agent(AgentError::NoResponse));
        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay_ms = retry_delay * (1u64 << (attempt - 1).min(5));
                warn!(
                    agent = %agent,
                    attempt,
                    max = max_retries,
                    delay_ms,
                    "⚠️ 流式 LLM 请求失败，{delay_ms}ms 后重试（{attempt}/{max_retries}）"
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            }
            stream_result = stream_chat(
                client.clone(),
                &model_name,
                messages.clone(),
                Some(0.7),
                Some(8192u32),
                tools_for_stream.clone(),
                None,
                response_format.clone(),
            )
            .await;
            match &stream_result {
                Ok(_) => {
                    if attempt > 0 {
                        info!(agent = %agent, attempt, "✅ 流式 LLM 重试成功");
                    }
                    break;
                }
                Err(e) if attempt < max_retries && is_retryable_llm_error(e) => {
                    warn!(agent = %agent, error = %e, "流式 LLM 可重试错误");
                }
                Err(_) => break,
            }
        }

        // 将 impl Stream 转换为 BoxStream<'static, ...>
        let stream = stream_result?;
        Ok(Box::pin(stream))
    }

    /// 处理流式响应的 chunk，收集内容并返回事件
    #[allow(clippy::type_complexity)]
    pub(crate) fn process_stream_chunk(
        chunk: &crate::llm::types::ChatCompletionChunk,
        content_buffer: &mut String,
        tool_call_map: &mut HashMap<u32, (String, String, String)>,
    ) -> Option<AgentEvent> {
        let mut event = None;

        if let Some(choice) = chunk.choices.first() {
            if let Some(content) = &choice.delta.content
                && !content.is_empty()
            {
                content_buffer.push_str(content);
                event = Some(AgentEvent::Token(content.clone()));
            }

            if let Some(delta_calls) = &choice.delta.tool_calls {
                for dc in delta_calls {
                    let entry = tool_call_map
                        .entry(dc.index)
                        .or_insert_with(|| (String::new(), String::new(), String::new()));
                    if let Some(id) = &dc.id
                        && !id.is_empty()
                    {
                        entry.0 = id.clone();
                    }
                    if let Some(f) = &dc.function {
                        if let Some(name) = &f.name
                            && !name.is_empty()
                        {
                            entry.1 = name.clone();
                        }
                        if let Some(args) = &f.arguments {
                            entry.2.push_str(args);
                        }
                    }
                }
            }
        }

        event
    }

    /// 将收集的 tool_call_map 转换为结构化的工具调用列表
    pub(crate) fn build_tool_calls_from_map(
        tool_call_map: &HashMap<u32, (String, String, String)>,
    ) -> (Vec<LlmToolCall>, Vec<(String, String, Value)>) {
        let mut sorted_indices: Vec<u32> = tool_call_map.keys().cloned().collect();
        sorted_indices.sort();

        let mut msg_tool_calls: Vec<LlmToolCall> = Vec::new();
        let mut steps: Vec<(String, String, Value)> = Vec::new();

        for idx in &sorted_indices {
            let (id, name, args_str) = &tool_call_map[idx];
            let args: Value =
                serde_json::from_str(args_str).unwrap_or(Value::Object(Default::default()));

            msg_tool_calls.push(LlmToolCall {
                id: id.clone(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: name.clone(),
                    arguments: args_str.clone(),
                },
            });
            steps.push((id.clone(), name.clone(), args));
        }

        (msg_tool_calls, steps)
    }

    /// 保存 checkpoint（用于 chat 模式）
    pub(crate) async fn save_checkpoint(&self) {
        if let (Some(cp), Some(tid)) = (&self.checkpointer, self.config.session_id.clone()) {
            let messages = self.context.messages().to_vec();
            if let Err(e) = cp.put(&tid, messages).await {
                tracing::warn!(
                    agent = %self.config.agent_name,
                    error = %e,
                    "⚠️ Checkpoint 保存失败"
                );
            }
        }
    }

    /// 流式执行的统一入口
    ///
    /// 根据 `mode` 参数决定：
    /// - `StreamMode::Execute`：重置上下文，从 checkpoint 恢复，适合单轮任务
    /// - `StreamMode::Chat`：保留上下文，适合多轮对话
    pub(crate) async fn run_stream(
        &mut self,
        input: &str,
        mode: StreamMode,
    ) -> Result<futures::stream::BoxStream<'_, Result<AgentEvent>>> {
        let input = input.to_string();
        let stream = async_stream::try_stream! {
            let agent = self.config.agent_name.clone();
            let callbacks = self.config.callbacks.clone();

            // 初始化上下文
            let recalled = self.prepare_stream_context(mode, &input).await;

            // 根据模式输出不同的日志
            match mode {
                StreamMode::Execute => info!(agent = %agent, "🌊 Agent 开始流式执行任务"),
                StreamMode::Chat => info!(agent = %agent, "🌊 Agent 开始流式多轮对话"),
            }

            if recalled > 0 {
                yield AgentEvent::MemoryRecalled { count: recalled };
            }

            for iteration in 0..self.config.max_iterations {
                for cb in &callbacks {
                    cb.on_iteration(&agent, iteration).await;
                }

                debug!(agent = %agent, iteration = iteration + 1, "--- 流式迭代 ---");

                let messages = self.context.prepare(None).await?;

                for cb in &callbacks {
                    cb.on_think_start(&agent, &messages).await;
                }

                yield AgentEvent::ThinkStart;

                // 创建 LLM 流
                let llm_stream = self.create_llm_stream(messages.clone()).await?;
                let mut llm_stream = Box::pin(llm_stream);

                // 收集流式响应
                let mut content_buffer = String::new();
                let mut tool_call_map: HashMap<u32, (String, String, String)> = HashMap::new();

                while let Some(chunk_result) = llm_stream.next().await {
                    let chunk = chunk_result?;
                    if let Some(event) = Self::process_stream_chunk(&chunk, &mut content_buffer, &mut tool_call_map) {
                        yield event;
                    }
                }

                yield AgentEvent::ThinkEnd {
                    tokens_used: content_buffer.len() / 4 + 1,
                };

                // 判断是否有工具调用
                let has_tool_calls = !tool_call_map.is_empty();

                if has_tool_calls {
                    // 构建工具调用
                    let (msg_tool_calls, steps) = Self::build_tool_calls_from_map(&tool_call_map);

                    // 发出 ToolCall 事件
                    for (_, name, args) in &steps {
                        yield AgentEvent::ToolCall {
                            name: name.clone(),
                            args: args.clone(),
                        };
                    }

                    // 触发 on_think_end 回调
                    {
                        let think_steps: Vec<StepType> = steps.iter().map(|(id, name, args)| {
                            StepType::Call {
                                tool_call_id: id.clone(),
                                function_name: name.clone(),
                                arguments: args.clone(),
                            }
                        }).collect();
                        for cb in &callbacks {
                            cb.on_think_end(&agent, &think_steps).await;
                        }
                    }

                    // 将 assistant 消息推送到上下文
                    self.context.push(Message::assistant_with_tools(msg_tool_calls));

                    // 执行工具调用并 yield 事件
                    let mut done = false;
                    for (tool_call_id, function_name, arguments) in steps {
                        let result = self.execute_tool_feedback(&function_name, &arguments).await?;

                        yield AgentEvent::ToolResult {
                            name: function_name.clone(),
                            output: result.clone(),
                        };

                        self.context.push(Message::tool_result(
                            tool_call_id,
                            function_name.clone(),
                            result.clone(),
                        ));

                        if function_name == TOOL_FINAL_ANSWER {
                            for cb in &callbacks {
                                cb.on_final_answer(&agent, &result).await;
                            }
                            info!(agent = %agent, "🏁 流式执行完成");

                            // Chat 模式保存 checkpoint
                            if mode == StreamMode::Chat {
                                self.save_checkpoint().await;
                            }

                            yield AgentEvent::FinalAnswer(result);
                            done = true;
                            break;
                        }
                    }

                    if done {
                        return;
                    }
                } else if !content_buffer.is_empty() {
                    // 纯文本响应
                    let think_steps = vec![StepType::Thought(content_buffer.clone())];
                    for cb in &callbacks {
                        cb.on_think_end(&agent, &think_steps).await;
                    }
                    for cb in &callbacks {
                        cb.on_final_answer(&agent, &content_buffer).await;
                    }
                    self.context.push(Message::assistant(content_buffer.clone()));

                    // Chat 模式保存 checkpoint
                    if mode == StreamMode::Chat {
                        self.save_checkpoint().await;
                    }

                    yield AgentEvent::FinalAnswer(content_buffer);
                    return;
                } else {
                    Err(ReactError::Agent(AgentError::NoResponse))?;
                }
            }

            Err(ReactError::Agent(AgentError::MaxIterationsExceeded(
                self.config.max_iterations,
            )))?;
        };

        Ok(Box::pin(stream))
    }
}
