//! Tool execution (invocation, guards, truncation)

use super::super::{ReactAgent, TOOL_FINAL_ANSWER};
use super::context::HookMessageBatches;
use crate::error::{ReactError, Result, ToolError};
use crate::guard::GuardDirection;
use crate::tools::{ToolParameters, ToolStreamEvent, is_read_tool, is_write_tool};
use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

#[allow(dead_code)]
pub(crate) struct ToolExecutionOutcome {
    pub output: String,
    pub tool_result: Option<crate::tools::ToolResult>,
    pub hook_messages: HookMessageBatches,
}

pub(crate) struct ToolExecutionFailure {
    pub error: ReactError,
    pub hook_messages: HookMessageBatches,
}

impl ReactAgent {
    #[tracing::instrument(skip(self, input), fields(agent = %self.config.agent_name, tool.name = %tool_name))]
    pub(crate) fn execute_tool_feedback_raw<'a>(
        &'a self,
        tool_name: &'a str,
        input: &'a Value,
        soften_errors: bool,
    ) -> BoxFuture<'a, std::result::Result<ToolExecutionOutcome, ToolExecutionFailure>> {
        Box::pin(async move {
            // If a custom pipeline is configured, delegate to it
            if let Some(ref pipeline) = self.tool_execution_pipeline {
                return self
                    .execute_with_pipeline(tool_name, input, soften_errors, pipeline)
                    .await;
            }

            // ── Inline implementation (fallback when no pipeline configured) ──
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

            // ── Intervention callbacks (highest-priority decision point) ──
            // Check interventions before hooks and approval, allowing
            // external observers to block/redirect/modify/cancel the tool call.
            for intervention in &self.tools.intervention_callbacks {
                let result = intervention
                    .on_tool_call(&self.config.agent_name, tool_name, input)
                    .await;
                if result.cancel {
                    return Err(ToolExecutionFailure {
                        error: ReactError::Other(format!(
                            "Agent execution cancelled by intervention: tool {}",
                            tool_name
                        )),
                        hook_messages,
                    });
                }
                if result.block {
                    let reason = result
                        .block_reason
                        .unwrap_or_else(|| "blocked by intervention callback".into());
                    info!(agent = %agent, tool = %tool_name, reason = %reason, "Intervention blocked tool");
                    return Ok(ToolExecutionOutcome {
                        tool_result: None,
                        output: format!("Tool {} blocked by intervention: {}", tool_name, reason),
                        hook_messages,
                    });
                }
                // If the intervention redirects to a different tool, we recursively
                // call execute_tool_feedback_raw with the new tool name and (possibly) modified args.
                if let Some(redirect) = result.redirect_to {
                    let redirect_args = result.modified_args.unwrap_or_else(|| input.clone());
                    info!(agent = %agent, from = %tool_name, to = %redirect, "Intervention redirected tool");
                    return self
                        .execute_tool_feedback_raw(&redirect, &redirect_args, soften_errors)
                        .await;
                }
                // If the intervention modifies args without redirect, update the input
                // and continue with the rest of the pipeline using modified args.
                if let Some(modified) = result.modified_args {
                    // We'll apply the modified args below by overwriting effective_params
                    // after the hook phase. For now, update the input that hooks see.
                    // Since we haven't entered the hook phase yet, just update input directly.
                    return self
                        .execute_tool_feedback_raw(tool_name, &modified, soften_errors)
                        .await;
                }
                // If the intervention injects context, add it to the hook_messages
                // so it gets fed back into the agent's reasoning after this tool call.
                if let Some(context) = result.injected_context {
                    hook_messages.pre.push(context);
                }
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
                        tool_result: None,
                        output: format!("Tool {} blocked by hook: {}", tool_name, reason),
                        hook_messages,
                    });
                }

                if let Some(updated) = hook_result.updated_input {
                    hook_modified_input = updated.clone();
                    if let Value::Object(map) = &updated {
                        effective_params =
                            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    }
                }
            }

            // ── Unified approval check ──
            // PermissionService
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

            // ── Safety notice for destructive tools ──
            if is_write_tool(tool_name) || tool_name == "shell" || tool_name == "delete_file" {
                let risk = match tool_name {
                    "shell" => "Command execution",
                    "delete_file" => "File deletion",
                    _ => "File modification",
                };
                let path = extract_path_param(tool_name, &effective_params)
                    .unwrap_or_else(|| "unknown".into());
                info!(
                    agent = %agent,
                    tool = %tool_name,
                    path = %path,
                    risk = %risk,
                    "Safety: {tool_name} will {risk} at {path}"
                );
                self.record_trace_event(crate::trace::RunEvent::PermissionDecision {
                    tool: tool_name.to_string(),
                    decision: "allow".into(),
                    reason: format!("{risk} at {path}"),
                })
                .await;
            }

            // ── Read-before-edit enforcement ──
            if self.config.force_read_before_edit
                && is_write_tool(tool_name)
                && let Some(path) = extract_path_param(tool_name, &effective_params)
            {
                let canonical = std::fs::canonicalize(&path)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| path.clone());
                if !self.was_file_read(&canonical) {
                    let msg = format!(
                        "Read-before-edit is enabled. File '{}' has not been read in this conversation turn. \
                         Use read_file to read it first, then retry this operation.",
                        path
                    );
                    warn!(agent = %agent, tool = %tool_name, path = %path, "Read-before-edit rejected");
                    return Ok(ToolExecutionOutcome {
                        tool_result: None,
                        output: msg,
                        hook_messages,
                    });
                }
            }

            let execution_start = std::time::Instant::now();

            let call_id = format!("call_{}", uuid::Uuid::new_v4());
            // Record ToolCall trace event (redaction handled by new_tool_call)
            self.record_trace_event(crate::trace::RunEvent::new_tool_call(
                call_id.clone(),
                tool_name.to_string(),
                Some(input.clone()),
                None,
                0,
            ))
            .await;

            // ── Execute tool (streaming or standard path) ──
            let result = if self.tools.tool_manager.supports_streaming(tool_name) {
                // Streaming path: call execute_tool_stream_collect and process
                // intermediate events, then extract the final ToolResult from
                // the Complete event.
                info!(agent = %agent, tool = %tool_name, "Streaming tool execution");
                let stream_events = match self
                    .tools
                    .tool_manager
                    .execute_tool_stream_collect(tool_name, effective_params.clone())
                    .await
                {
                    Ok(events) => events,
                    Err(error) => {
                        let error_msg = error.to_string();
                        warn!(agent = %agent, tool = %tool_name, error = %error_msg, "Streaming tool execution failed");

                        self.record_trace_event(crate::trace::RunEvent::ToolError {
                            call_id: call_id.clone(),
                            name: tool_name.to_string(),
                            message: error_msg.clone(),
                        })
                        .await;

                        for cb in &callbacks {
                            cb.on_tool_error(&agent, tool_name, &error).await;
                        }
                        self.log_tool_call_audit(tool_name, input, &error_msg, false, 0)
                            .await;

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
                                "Streaming tool error converted to observation"
                            );
                            return Ok(ToolExecutionOutcome {
                                tool_result: None,
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

                // Process intermediate stream events (Progress / PartialOutput)
                // and extract the final ToolResult from the Complete event.
                let mut final_result: Option<crate::tools::ToolResult> = None;
                for event in &stream_events {
                    match event {
                        ToolStreamEvent::Progress { message, percent } => {
                            debug!(
                                agent = %agent,
                                tool = %tool_name,
                                message = %message,
                                percent = ?percent,
                                "Streaming tool progress"
                            );
                        }
                        ToolStreamEvent::PartialOutput { chunk } => {
                            debug!(
                                agent = %agent,
                                tool = %tool_name,
                                chunk_len = chunk.len(),
                                "Streaming tool partial output"
                            );
                        }
                        ToolStreamEvent::Complete(r) => {
                            final_result = Some(r.clone());
                        }
                    }
                }

                // If no Complete event was received, treat it as an execution failure
                match final_result {
                    Some(r) => r,
                    None => {
                        let msg = format!(
                            "Streaming tool '{}' completed without a final result event",
                            tool_name
                        );
                        let error = ReactError::from(ToolError::ExecutionFailed {
                            tool: tool_name.to_string(),
                            message: msg.clone(),
                        });
                        return Err(ToolExecutionFailure {
                            error,
                            hook_messages,
                        });
                    }
                }
            } else {
                // Standard (non-streaming) path
                match self
                    .tools
                    .tool_manager
                    .execute_tool(tool_name, effective_params.clone())
                    .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        // Apply softening logic for tool execution errors (connection failures etc.)
                        // so transient MCP/network errors don't terminate the agent stream.
                        let error_msg = error.to_string();
                        warn!(agent = %agent, tool = %tool_name, error = %error_msg, "Tool execution failed");

                        // Record ToolError trace event
                        self.record_trace_event(crate::trace::RunEvent::ToolError {
                            call_id: call_id.clone(),
                            name: tool_name.to_string(),
                            message: error_msg.clone(),
                        })
                        .await;

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
                                "Tool error converted to observation and sent back to LLM"
                            );
                            return Ok(ToolExecutionOutcome {
                                tool_result: None,
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
                }
            };
            let duration_ms = execution_start.elapsed().as_millis() as u64;

            // Record ToolResult trace event
            self.record_trace_event(crate::trace::RunEvent::ToolResult {
                call_id: call_id.clone(),
                name: tool_name.to_string(),
                success: true,
                output_preview: Some(result.output.chars().take(200).collect()),
                output_truncated: false,
                duration_ms,
            })
            .await;

            // ── Record file reads for read-before-edit enforcement ──
            if result.success
                && is_read_tool(tool_name)
                && let Some(path) = extract_path_param(tool_name, &effective_params)
            {
                let canonical = std::fs::canonicalize(&path)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| path.clone());
                self.record_file_read(&canonical);
            }

            // Record FileEdit trace event for write tools
            if result.success
                && is_write_tool(tool_name)
                && let Some(path) = extract_path_param(tool_name, &effective_params)
            {
                self.record_trace_event(crate::trace::RunEvent::FileEdit {
                    tool: tool_name.to_string(),
                    path,
                })
                .await;
            }

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
                        tool_result: None,
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
                        tool_result: None,
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
                    tool_result: None,
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
                        error = %err,
                        "⚠️ Tool error converted to observation and sent back to LLM"
                    );
                    Ok(ToolExecutionOutcome {
                        tool_result: None,
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
        })
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
    #[allow(dead_code)]
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
    /// exceed the limit, the output is truncated to a **head + tail** view:
    /// the first ~70% of the budget is taken from the beginning, the remaining
    /// ~30% from the end.  Cut points are aligned to newline boundaries so that
    /// code blocks, JSON structures, and log lines stay intact.
    pub(crate) async fn truncate_tool_output(&self, output: String) -> String {
        // Only apply truncation/summary when a max token limit is configured
        let Some(max_tokens) = self.config.max_tool_output_tokens else {
            return output;
        };

        let ctx = self.memory.context.lock().await;
        let tokenizer = ctx.tokenizer();
        let token_count = tokenizer.count_tokens(&output);
        if token_count <= max_tokens {
            drop(ctx);
            return output;
        }
        drop(ctx);

        // Event-level summary: for long tool outputs (>2000 chars), generate a structured summary
        const SUMMARY_THRESHOLD: usize = 2000;
        if output.len() > SUMMARY_THRESHOLD {
            let line_count = output.lines().count();
            let char_count = output.len();
            let first_line = output.lines().next().unwrap_or("");
            let summary = format!(
                "[Summary: {char_count} chars, ~{line_count} lines. First line: {first_line}]\n\n"
            );
            let head: String = output.chars().skip(first_line.len()).take(1400).collect();
            let tail: String = output
                .chars()
                .rev()
                .take(400)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            return format!("{summary}{head}\n...\n{tail}");
        }

        // Large output spill-to-disk: if output exceeds 1MB, write to temp file
        const SPILL_THRESHOLD: usize = 1_048_576; // 1MB
        if output.len() > SPILL_THRESHOLD {
            let tmp_dir = std::env::temp_dir().join("echo_agent_spill");
            let _ = std::fs::create_dir_all(&tmp_dir);
            match tempfile::NamedTempFile::new_in(&tmp_dir) {
                Ok(mut tmp) => {
                    use std::io::Write;
                    if tmp.write_all(output.as_bytes()).is_ok() {
                        // Persist the temp file so it can be read later
                        match tmp.keep() {
                            Ok((_, path)) => {
                                let preview: String = output.chars().take(500).collect();
                                return format!(
                                    "{preview}\n\n[Output spilled to disk: {} ({:.1}MB). Use read_file to read the full output.]",
                                    path.display(),
                                    output.len() as f64 / 1_048_576.0
                                );
                            }
                            Err(_) => { /* keep failed, fall through to truncation */ }
                        }
                    }
                }
                Err(_) => { /* temp file creation failed, fall through to truncation */ }
            }
        }

        let Some(max_tokens) = self.config.max_tool_output_tokens else {
            return output;
        };
        let ctx = self.memory.context.lock().await;
        let tokenizer = ctx.tokenizer();
        let token_count = tokenizer.count_tokens(&output);
        if token_count <= max_tokens {
            drop(ctx);
            return output;
        }

        let notice = format!(
            "\n\n[... output truncated: {} tokens total → {} tokens shown ...]\n\n",
            token_count, max_tokens,
        );
        let notice_tokens = tokenizer.count_tokens(&notice);
        let available = max_tokens.saturating_sub(notice_tokens);

        // If the budget is too tight for a meaningful split, fall back to
        // prefix-only with a short suffix.
        if available < 4 {
            drop(ctx);
            let truncated: String = output.chars().take(max_tokens * 4).collect();
            return format!("{truncated}\n[Output truncated, total {token_count} tokens]");
        }

        let head_budget = (available as f64 * 0.7) as usize;
        let tail_budget = available.saturating_sub(head_budget);

        let chars: Vec<char> = output.chars().collect();
        let char_per_token = chars.len() as f64 / token_count as f64;

        // ── head ──────────────────────────────────────────────────
        let head_char_end = {
            let est = (head_budget as f64 * char_per_token * 1.05) as usize;
            newline_boundary_fwd(&chars, est.min(chars.len()))
        };
        let head: String = chars[..head_char_end].iter().collect();
        let actual_head = tokenizer.count_tokens(&head);
        let head = if actual_head > head_budget {
            let scale = head_budget as f64 / actual_head as f64;
            let adj = newline_boundary_fwd(&chars, (head_char_end as f64 * scale) as usize);
            chars[..adj].iter().collect::<String>()
        } else {
            head
        };

        // ── tail ──────────────────────────────────────────────────
        let tail_char_start = {
            let est = chars
                .len()
                .saturating_sub((tail_budget as f64 * char_per_token * 1.05) as usize);
            newline_boundary_rev(&chars, est)
        };
        let tail: String = chars[tail_char_start..].iter().collect();
        let actual_tail = tokenizer.count_tokens(&tail);
        let tail = if actual_tail > tail_budget {
            let scale = tail_budget as f64 / actual_tail as f64;
            let keep = (tail.len() as f64 * scale) as usize;
            let adj =
                newline_boundary_rev(&chars, tail_char_start + tail.len().saturating_sub(keep));
            chars[adj..].iter().collect::<String>()
        } else {
            tail
        };

        drop(ctx);
        format!("{head}{notice}{tail}")
    }

    /// Perform guard check on tool output to prevent malicious content injection
    ///
    /// If a guard manager is configured, output is checked for safety.
    /// Returns `Some(filtered_output)` if output was filtered/modified,
    /// returns `None` if output is fine and needs no modification.
    pub(crate) async fn check_tool_output_guard(&self, output: &str) -> Option<String> {
        // Secret scan: redact secrets from tool output before guard check
        if crate::security::contains_secrets(output) {
            let redacted = crate::security::redact_secrets(output);
            warn!(agent = %self.config.agent_name, "Secret detected in tool output; redacted");
            return Some(redacted);
        }
        let gm = self.guard.guard_manager.as_ref()?;
        let result = match gm.check_all(output, GuardDirection::Output).await {
            Ok(r) => r,
            Err(e) => {
                error!(agent = %self.config.agent_name, error = %e, "Guard check failed, blocking output (fail-closed)");
                return Some(format!("Output content blocked: guard check error ({e})"));
            }
        };
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

    /// Execute a tool through the configured pipeline.
    async fn execute_with_pipeline(
        &self,
        tool_name: &str,
        input: &Value,
        soften_errors: bool,
        pipeline: &Arc<crate::agent::react::run::pipeline::ToolExecutionPipeline>,
    ) -> std::result::Result<ToolExecutionOutcome, ToolExecutionFailure> {
        let _agent_name = self.config.agent_name.clone();
        let params: ToolParameters = if let Value::Object(map) = input {
            map.clone().into_iter().collect()
        } else {
            HashMap::new()
        };

        let mut ctx = crate::agent::react::run::pipeline::ToolExecutionContext {
            call_id: format!("call_{}", uuid::Uuid::new_v4()),
            tool_name: tool_name.to_string(),
            params,
            input: input.clone(),
            hook_messages: HookMessageBatches::default(),
            result: None,
            output: None,
            blocked: false,
            block_reason: None,
            duration_ms: 0,
            plan_mode: self.config.plan_mode,
        };

        match pipeline.run(&mut ctx, self).await {
            Ok(()) => {
                if ctx.blocked {
                    return Ok(ToolExecutionOutcome {
                        tool_result: None,
                        output: ctx
                            .block_reason
                            .unwrap_or_else(|| format!("Tool {} blocked", tool_name)),
                        hook_messages: ctx.hook_messages,
                    });
                }

                if let Some(result) = ctx.result {
                    if result.success {
                        let output = ctx.output.unwrap_or_else(|| result.output.clone());
                        Ok(ToolExecutionOutcome {
                            tool_result: Some(result),
                            output,
                            hook_messages: ctx.hook_messages,
                        })
                    } else {
                        let error_msg = result
                            .error
                            .clone()
                            .unwrap_or_else(|| result.output.clone());
                        let err = ReactError::from(ToolError::ExecutionFailed {
                            tool: tool_name.to_string(),
                            message: error_msg.clone(),
                        });
                        if soften_errors && tool_name != TOOL_FINAL_ANSWER {
                            Ok(ToolExecutionOutcome {
                                tool_result: Some(result),
                                output: format!(
                                    "[Tool execution failed] {err}\nTip: adjust parameters based on the error and retry, or try other tools."
                                ),
                                hook_messages: ctx.hook_messages,
                            })
                        } else {
                            Err(ToolExecutionFailure {
                                error: err,
                                hook_messages: ctx.hook_messages,
                            })
                        }
                    }
                } else {
                    Err(ToolExecutionFailure {
                        error: ReactError::Other("Pipeline completed without result".into()),
                        hook_messages: ctx.hook_messages,
                    })
                }
            }
            Err(error) => Err(ToolExecutionFailure {
                error,
                hook_messages: ctx.hook_messages,
            }),
        }
    }

    /// Execute tool, deciding failure behavior based on `tool_error_feedback` config:
    /// - `true` (default): convert error info to a tool observation sent back to LLM so the model can self-correct
    /// - `false`: propagate `Err` upwards directly, matching legacy behavior
    ///
    /// The `final_answer` tool always preserves original error semantics and is never softened.
    /// Tool output goes through `truncate_tool_output` for token budget truncation.
    #[allow(dead_code)]
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

// ── truncation helpers ──────────────────────────────────────────────

/// Find the nearest newline at or before `target`, so truncation lands on
/// a natural boundary (line / code-block / JSON key end).
fn newline_boundary_fwd(chars: &[char], target: usize) -> usize {
    let t = target.min(chars.len());
    for i in (0..t).rev() {
        if chars[i] == '\n' {
            return i + 1; // include the newline in the kept portion
        }
    }
    t
}

/// Find the nearest newline at or after `target`, so the tail starts at a
/// clean line boundary.
fn newline_boundary_rev(chars: &[char], target: usize) -> usize {
    let t = target.min(chars.len());
    for (i, ch) in chars.iter().enumerate().skip(t) {
        if *ch == '\n' {
            return i + 1; // start after the newline
        }
    }
    t
}

// ── Read-before-edit helpers ─────────────────────────────────────────────────

/// Extract the target file path from the tool parameters.
/// Looks for `path` or `file_path` keys common across file tools.
fn extract_path_param(_tool_name: &str, params: &ToolParameters) -> Option<String> {
    params
        .get("path")
        .or_else(|| params.get("file_path"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}
