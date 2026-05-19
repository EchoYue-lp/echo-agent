//! Tool execution (invocation, guards, truncation)

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

        info!(agent = %agent, tool = %tool_name, "🔧 Starting tool execution");
        debug!(agent = %agent, tool = %tool_name, params = %input, "Tool parameter details");

        // ── PreToolUse hooks (execute before approval, allow hook to intercept or modify params) ──
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

        // ── Unified approval check ──
        // PermissionService → PermissionPolicy
        // Returns the user-modified parameters during approval (if any)
        let approval_modified_args = self
            .check_tool_approval(tool_name, &hook_modified_input)
            .await
            .map_err(|error| ToolExecutionFailure {
                error,
                hook_messages: hook_messages.clone(),
            })?;

        // If the user modified parameters during approval, override actual execution parameters
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
                warn!(agent = %agent, tool = %tool_name, error = %error_msg, "💥 Tool execution failed");
                for cb in &callbacks {
                    cb.on_tool_error(&agent, tool_name, &error).await;
                }
                self.log_tool_call_audit(tool_name, input, &error_msg, false, 0)
                    .await;

                // ── PostToolUseFailure hook ──
                self.run_post_failure_hook(
                    &agent,
                    tool_name,
                    input,
                    &error_msg,
                    &mut hook_messages,
                )
                .await?;

                if soften_errors && tool_name != TOOL_FINAL_ANSWER {
                    warn!(
                        agent = %agent,
                        tool = %tool_name,
                        error = %error,
                        "⚠️ Tool error converted to observation and sent back to LLM"
                    );
                    return Ok(ToolExecutionOutcome {
                        output: format!(
                            "[Tool execution failed] {error}\nTip: adjust parameters based on the error and retry, or try other tools."
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
            if post_result.block {
                info!(agent = %agent, tool = %tool_name, reason = ?post_result.block_reason, "PostToolUse hook blocked tool output");
                let blocked_output = post_result
                    .block_reason
                    .unwrap_or_else(|| format!("Tool {} output blocked by hook", tool_name));
                return Ok(ToolExecutionOutcome {
                    output: blocked_output,
                    hook_messages,
                });
            }
        }

        if result.success {
            info!(agent = %agent, tool = %tool_name, "📤 Tool executed successfully");
            debug!(agent = %agent, tool = %tool_name, output = %result.output, "Tool output details");

            // Run output guard checks to prevent malicious content injection
            if let Some(guard_output) = self.check_tool_output_guard(&result.output).await {
                debug!(agent = %agent, tool = %tool_name, "🛡️ Tool output filtered by guard");
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
            warn!(agent = %agent, tool = %tool_name, error = %error_msg, "💥 Tool execution failed");
            let err = ReactError::from(ToolError::ExecutionFailed {
                tool: tool_name.to_string(),
                message: error_msg.clone(),
            });
            for cb in &callbacks {
                cb.on_tool_error(&agent, tool_name, &err).await;
            }
            self.log_tool_call_audit(tool_name, input, &error_msg, false, duration_ms)
                .await;

            // ── PostToolUseFailure hook ──
            self.run_post_failure_hook(&agent, tool_name, input, &error_msg, &mut hook_messages)
                .await?;

            if soften_errors && tool_name != TOOL_FINAL_ANSWER {
                warn!(
                    agent = %agent,
                    tool = %tool_name,
                    error = %err,
                    "⚠️ Tool error converted to observation and sent back to LLM"
                );
                Ok(ToolExecutionOutcome {
                    output: format!(
                        "[Tool execution failed] {err}\nTip: adjust parameters based on the error and retry, or try other tools."
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

    /// Run the PostToolUseFailure hook and check for block.
    ///
    /// Returns `Err(ToolExecutionFailure)` if a hook blocks the error output.
    async fn run_post_failure_hook(
        &self,
        agent: &str,
        tool_name: &str,
        input: &Value,
        error_msg: &str,
        hook_messages: &mut HookMessageBatches,
    ) -> std::result::Result<(), ToolExecutionFailure> {
        let hook_reg = {
            let guard = self.tools.hook_registry.read().await;
            guard.clone()
        };
        let failure_result = hook_reg
            .run_post_tool_use_failure(
                tool_name,
                input,
                error_msg,
                self.config.get_session_id().unwrap_or(""),
            )
            .await;
        hook_messages.post = failure_result.messages;
        if failure_result.block {
            info!(agent = %agent, tool = %tool_name, reason = ?failure_result.block_reason, "PostToolUseFailure hook blocked error output");
            let blocked_msg = failure_result
                .block_reason
                .unwrap_or_else(|| format!("Tool {} error output blocked by hook", tool_name));
            return Err(ToolExecutionFailure {
                error: ReactError::Other(blocked_msg),
                hook_messages: hook_messages.clone(),
            });
        }
        Ok(())
    }

    /// Execute tool, preserving the real error information returned by the tool
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

    /// Truncate tool output based on token budget.
    ///
    /// When `max_tool_output_tokens` is configured and the estimated output tokens
    /// exceed the limit, truncate the text and append a `[Output truncated, total N tokens]` notice.
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

        // Estimate truncation position by character ratio
        let ratio = max_tokens as f64 / token_count as f64;
        let char_limit = (output.len() as f64 * ratio * 0.95) as usize;
        let truncated: String = output.chars().take(char_limit).collect();
        let suffix = format!(
            "\n[Output truncated, total {} tokens, keeping first {} tokens]",
            token_count, max_tokens
        );
        format!("{truncated}{suffix}")
    }

    /// Perform guard check on tool output to prevent malicious content injection
    ///
    /// If a guard manager is configured, output is checked for safety.
    /// Returns `Some(filtered_output)` if output was filtered/modified,
    /// returns `None` if output is fine and needs no modification.
    pub(crate) async fn check_tool_output_guard(&self, output: &str) -> Option<String> {
        let gm = self.guard.guard_manager.as_ref()?;
        let result = gm.check_all(output, GuardDirection::Output).await.ok()?;
        if let crate::guard::GuardResult::Block { reason } = &result {
            info!(agent = %self.config.agent_name, reason = %reason, "🛡️ Tool output blocked by guard");
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
            Some(format!("Output content filtered by safety guard: {reason}"))
        } else {
            None
        }
    }

    /// Execute tool, deciding failure behavior based on `tool_error_feedback` config:
    /// - `true` (default): convert error info to a tool observation sent back to LLM so the model can self-correct
    /// - `false`: propagate `Err` upwards directly, matching legacy behavior
    ///
    /// The `final_answer` tool always preserves original error semantics and is never softened.
    /// Tool output goes through `truncate_tool_output` for token budget truncation.
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
