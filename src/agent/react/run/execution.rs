//! 工具执行（调用、护栏、截断）

use super::super::{ReactAgent, TOOL_FINAL_ANSWER};
use super::context::HookMessageBatches;
use crate::error::{ReactError, Result, ToolError};
use crate::guard::GuardDirection;
use crate::tools::ToolParameters;
use serde_json::Value;
use std::collections::HashMap;
use tracing::{debug, info, warn};

pub(crate) struct ToolExecutionOutcome {
    pub output: String,
    pub hook_messages: HookMessageBatches,
}

pub(crate) struct ToolExecutionFailure {
    pub error: ReactError,
    pub hook_messages: HookMessageBatches,
}

impl ReactAgent {
    #[tracing::instrument(skip(self, input), fields(agent = %self.config.agent_name, tool.name = %tool_name))]
    pub(crate) async fn execute_tool_feedback_raw(
        &self,
        tool_name: &str,
        input: &Value,
        soften_errors: bool,
    ) -> std::result::Result<ToolExecutionOutcome, ToolExecutionFailure> {
        let agent = self.config.agent_name.clone();
        let callbacks = self.config.callbacks.clone();
        let params: ToolParameters = if let Value::Object(map) = input {
            map.clone().into_iter().collect()
        } else {
            HashMap::new()
        };
        let mut hook_messages = HookMessageBatches::default();

        for cb in &callbacks {
            cb.on_tool_start(&agent, tool_name, input).await;
        }

        info!(agent = %agent, tool = %tool_name, "🔧 开始执行工具");
        debug!(agent = %agent, tool = %tool_name, params = %input, "工具参数详情");

        // ── PreToolUse hooks（审批前执行，允许 hook 拦截或修改参数）──
        let mut effective_params = params;
        let mut hook_modified_input = input.clone();
        let has_hooks = {
            let hook_reg = self.tools.hook_registry.read().await;
            !hook_reg.is_empty()
        };
        if has_hooks {
            // Clone registry to release lock BEFORE awaiting hooks.
            // Prevents deadlock when hook triggers nested tool calls
            // that re-enter execute_tool and try to acquire the same RwLock.
            let hook_reg = {
                let guard = self.tools.hook_registry.read().await;
                guard.clone()
            };
            let hook_result = hook_reg
                .run_pre_tool_use(tool_name, input, self.config.get_session_id().unwrap_or(""))
                .await;
            hook_messages.pre = hook_result.messages.clone();

            if hook_result.block {
                let reason = hook_result
                    .block_reason
                    .unwrap_or_else(|| "blocked by skill hook".into());
                info!(agent = %agent, tool = %tool_name, reason = %reason, "Hook blocked tool");
                return Ok(ToolExecutionOutcome {
                    output: format!("Tool {} blocked by hook: {}", tool_name, reason),
                    hook_messages,
                });
            }

            if let Some(updated) = hook_result.updated_input {
                hook_modified_input = updated.clone();
                if let Value::Object(map) = &updated {
                    effective_params = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                }
            }
        }

        // ── 统一审批检查 ──
        // PermissionService → PermissionPolicy
        // 返回用户在审批时修改的参数（如有）
        let approval_modified_args = self
            .check_tool_approval(tool_name, &hook_modified_input)
            .await
            .map_err(|error| ToolExecutionFailure {
                error,
                hook_messages: hook_messages.clone(),
            })?;

        // 如果用户在审批时修改了参数，覆盖工具的实际执行参数
        if let Some(modified) = approval_modified_args
            && let Value::Object(map) = &modified
        {
            effective_params = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        }

        let execution_start = std::time::Instant::now();
        let result = match self
            .tools
            .tool_manager
            .execute_tool(tool_name, effective_params)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                // Apply softening logic for tool execution errors (connection failures etc.)
                // so transient MCP/network errors don't terminate the agent stream.
                let error_msg = error.to_string();
                warn!(agent = %agent, tool = %tool_name, error = %error_msg, "💥 工具执行失败");
                for cb in &callbacks {
                    cb.on_tool_error(&agent, tool_name, &error).await;
                }
                self.log_tool_call_audit(tool_name, input, &error_msg, false, 0)
                    .await;
                if soften_errors && tool_name != TOOL_FINAL_ANSWER {
                    warn!(
                        agent = %agent,
                        tool = %tool_name,
                        error = %error,
                        "⚠️ 工具错误已转为观测值回传 LLM"
                    );
                    return Ok(ToolExecutionOutcome {
                        output: format!(
                            "[工具执行失败] {error}\n提示：请根据错误信息调整参数后重试，或换用其他工具。"
                        ),
                        hook_messages,
                    });
                } else {
                    return Err(ToolExecutionFailure {
                        error,
                        hook_messages,
                    });
                }
            }
        };
        let duration_ms = execution_start.elapsed().as_millis() as u64;

        // ── PostToolUse hooks ──
        let is_hook_post = {
            let hook_reg = self.tools.hook_registry.read().await;
            !hook_reg.is_empty()
        };
        if is_hook_post {
            // Clone registry to release lock BEFORE awaiting hooks (prevent deadlock).
            let hook_reg = {
                let guard = self.tools.hook_registry.read().await;
                guard.clone()
            };
            let post_result = hook_reg
                .run_post_tool_use(
                    tool_name,
                    input,
                    &result.output,
                    self.config.get_session_id().unwrap_or(""),
                )
                .await;
            hook_messages.post = post_result.messages;
        }

        if result.success {
            info!(agent = %agent, tool = %tool_name, "📤 工具执行成功");
            debug!(agent = %agent, tool = %tool_name, output = %result.output, "工具返回详情");

            // Run output guard checks to prevent malicious content injection
            if let Some(guard_output) = self.check_tool_output_guard(&result.output).await {
                debug!(agent = %agent, tool = %tool_name, "🛡️ 工具输出经护栏过滤");
                for cb in callbacks.iter() {
                    cb.on_tool_end(&agent, tool_name, &guard_output).await;
                }
                self.log_tool_call_audit(tool_name, input, &guard_output, true, duration_ms)
                    .await;
                return Ok(ToolExecutionOutcome {
                    output: guard_output,
                    hook_messages,
                });
            }

            for cb in callbacks.iter() {
                cb.on_tool_end(&agent, tool_name, &result.output).await;
            }
            self.log_tool_call_audit(tool_name, input, &result.output, true, duration_ms)
                .await;
            Ok(ToolExecutionOutcome {
                output: result.output,
                hook_messages,
            })
        } else {
            let error_msg = result
                .error
                .clone()
                .unwrap_or_else(|| result.output.clone());
            warn!(agent = %agent, tool = %tool_name, error = %error_msg, "💥 工具执行失败");
            let err = ReactError::from(ToolError::ExecutionFailed {
                tool: tool_name.to_string(),
                message: error_msg.clone(),
            });
            for cb in &callbacks {
                cb.on_tool_error(&agent, tool_name, &err).await;
            }
            self.log_tool_call_audit(tool_name, input, &error_msg, false, duration_ms)
                .await;
            if soften_errors && tool_name != TOOL_FINAL_ANSWER {
                warn!(
                    agent = %agent,
                    tool = %tool_name,
                    error = %err,
                    "⚠️ 工具错误已转为观测值回传 LLM"
                );
                Ok(ToolExecutionOutcome {
                    output: format!(
                        "[工具执行失败] {err}\n提示：请根据错误信息调整参数后重试，或换用其他工具。"
                    ),
                    hook_messages,
                })
            } else {
                Err(ToolExecutionFailure {
                    error: err,
                    hook_messages,
                })
            }
        }
    }

    /// 执行工具，保留工具返回的真实错误信息
    pub(crate) async fn execute_tool(&self, tool_name: &str, input: &Value) -> Result<String> {
        match self
            .execute_tool_feedback_raw(tool_name, input, false)
            .await
        {
            Ok(outcome) => {
                self.apply_hook_messages(tool_name, &outcome.hook_messages)
                    .await;
                Ok(outcome.output)
            }
            Err(failure) => {
                self.apply_hook_messages(tool_name, &failure.hook_messages)
                    .await;
                Err(failure.error)
            }
        }
    }

    /// 对工具输出进行 token 预算截断。
    ///
    /// 当 `max_tool_output_tokens` 已配置且输出估算 token 超限时，
    /// 截断文本并在尾部追加 `[输出已截断，共 N tokens]` 提示。
    pub(crate) async fn truncate_tool_output(&self, output: String) -> String {
        let Some(max_tokens) = self.config.max_tool_output_tokens else {
            return output;
        };
        let ctx = self.memory.context.lock().await;
        let tokenizer = ctx.tokenizer();
        let token_count = tokenizer.count_tokens(&output);
        drop(ctx);
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

    /// 对工具输出进行护栏检查，防止恶意内容注入
    ///
    /// 如果配置了护栏管理器，会对输出进行安全检查。
    /// 返回 `Some(filtered_output)` 表示输出被过滤/修改，
    /// 返回 `None` 表示输出正常，无需修改。
    pub(crate) async fn check_tool_output_guard(&self, output: &str) -> Option<String> {
        let gm = self.guard.guard_manager.as_ref()?;
        let result = gm.check_all(output, GuardDirection::Output).await.ok()?;
        if let crate::guard::GuardResult::Block { reason } = &result {
            info!(agent = %self.config.agent_name, reason = %reason, "🛡️ 工具输出被护栏阻断");
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
                let _ = al.log(event).await;
            }
            Some(format!("输出内容已被安全护栏过滤: {reason}"))
        } else {
            None
        }
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
        match self
            .execute_tool_feedback_raw(tool_name, input, self.config.tool_error_feedback)
            .await
        {
            Ok(outcome) => {
                self.apply_hook_messages(tool_name, &outcome.hook_messages)
                    .await;
                Ok(self.truncate_tool_output(outcome.output).await)
            }
            Err(failure) => {
                self.apply_hook_messages(tool_name, &failure.hook_messages)
                    .await;
                Err(failure.error)
            }
        }
    }
}
