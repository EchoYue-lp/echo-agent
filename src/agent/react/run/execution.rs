//! Tool execution (invocation, guards, truncation)
//!
//! All tool calls flow through the [`ToolExecutionPipeline`] (13-stage middleware).
//! When no custom pipeline is configured, a default pipeline is created automatically.

use super::super::{ReactAgent, TOOL_FINAL_ANSWER};
use super::context::HookMessageBatches;
use crate::error::{ReactError, Result, ToolError};
use crate::guard::GuardDirection;
use crate::tools::{ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{error, info, warn};

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
    fn summarize_trigger_text(value: impl AsRef<str>, max_chars: usize) -> String {
        let text = value.as_ref();
        let mut out: String = text.chars().take(max_chars).collect();
        if text.chars().count() > max_chars {
            out.push('…');
        }
        out
    }

    fn record_memory_trigger_tool_event(
        &self,
        tool_name: &str,
        input: &Value,
        result: Option<&ToolResult>,
        error: Option<&str>,
    ) {
        let Ok(mut state) = self.memory_trigger_state.lock() else {
            return;
        };

        let timestamp = crate::utils::time::now_local().to_rfc3339();
        let session_id = self.config.get_session_id().unwrap_or("").to_string();
        state
            .tool_sequences
            .push(crate::evolution::ToolSequenceRecord {
                tool_name: tool_name.to_string(),
                session_id,
                timestamp: timestamp.clone(),
            });
        if state.tool_sequences.len() > 100 {
            let excess = state.tool_sequences.len() - 100;
            state.tool_sequences.drain(0..excess);
        }

        let input_summary = Self::summarize_trigger_text(input.to_string(), 160);
        match (result, error) {
            (Some(result), _) if result.success => {
                if state.last_tool_failure.is_some() {
                    state.last_tool_success = Some(crate::evolution::ToolSuccessRecord {
                        tool_name: tool_name.to_string(),
                        input_summary,
                        output_summary: Self::summarize_trigger_text(&result.output, 160),
                        timestamp,
                    });
                }
            }
            (Some(result), _) => {
                state.last_tool_failure = Some(crate::evolution::ToolFailureRecord {
                    tool_name: tool_name.to_string(),
                    input_summary,
                    error: Self::summarize_trigger_text(
                        result.error.as_deref().unwrap_or(&result.output),
                        160,
                    ),
                    timestamp,
                });
                state.last_tool_success = None;
            }
            (None, Some(error)) => {
                state.last_tool_failure = Some(crate::evolution::ToolFailureRecord {
                    tool_name: tool_name.to_string(),
                    input_summary,
                    error: Self::summarize_trigger_text(error, 160),
                    timestamp,
                });
                state.last_tool_success = None;
            }
            (None, None) => {}
        }
    }

    /// Record skill telemetry for the currently activated skill(s).
    ///
    /// Fire-and-forget: spawns a detached task so a slow Store write never
    /// blocks the tool pipeline. No-op when no skill is activated or when no
    /// memory store is configured. Each activated skill gets its own record
    /// so `SkillHealthMonitor` / `SkillPatcher` / `SkillMerger` can pick up
    /// real usage data.
    fn record_skill_telemetry(
        &self,
        tool_name: &str,
        duration_ms: u64,
        success: bool,
        error: Option<&str>,
    ) {
        let Some(store) = self.memory.store.as_ref() else {
            return; // no store configured — graceful degradation
        };
        let activated = self.tools.skill_registry.activated_names();
        if activated.is_empty() {
            return; // no skill activated — skip
        }
        let session_id = self.config.get_session_id().unwrap_or("").to_string();
        let store = store.clone();
        let tool_name = tool_name.to_string();
        let error_msg = error.map(|e| e.to_string());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        tokio::spawn(async move {
            let ts = echo_state::skill_telemetry::SkillTelemetryStore::new(store);
            // Bridge: refresh curator last_used_at so apply_transitions computes
            // idle time from "last actual use" instead of "since creation".
            // touch_skill is sync + file-locked; errors are non-fatal.
            let curator = echo_agent::evolution::Curator::default_path(
                echo_agent::evolution::CuratorConfig::default(),
            );
            for skill_name in &activated {
                let record = echo_state::skill_telemetry::SkillExecutionRecord {
                    skill_name: skill_name.clone(),
                    session_id: session_id.clone(),
                    activated_at: now,
                    duration_ms,
                    tools_used: vec![tool_name.clone()],
                    tool_calls_count: 1,
                    success,
                    error_message: error_msg.clone(),
                };
                if let Err(e) = ts.record_execution(&record).await {
                    warn!(error = %e, skill = %skill_name, "skill telemetry write failed");
                }
                // P2-9: touch_skill 内部是 flock + fs read/write (curator.rs), 同步
                // 阻塞。此前在 async 块里直接调用会卡 tokio worker 线程。包进
                // spawn_blocking 丢到阻塞线程池 (best-effort, 忽略错误)。
                let curator_clone = curator.clone();
                let skill_clone = skill_name.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    curator_clone.touch_skill(&skill_clone, true)
                })
                .await;
            }
        });
    }

    #[tracing::instrument(skip(self, input), fields(agent = %self.config.agent_name, tool.name = %tool_name))]
    pub(crate) fn execute_tool_feedback_raw<'a>(
        &'a self,
        tool_name: &'a str,
        input: &'a Value,
        soften_errors: bool,
    ) -> BoxFuture<'a, std::result::Result<ToolExecutionOutcome, ToolExecutionFailure>> {
        Box::pin(async move {
            // Always use the pipeline path. If no custom pipeline is configured,
            // create a default one (13-stage middleware with sensible defaults).
            // This unifies the execution path and eliminates the inline fallback.
            let default_pipeline;
            let pipeline: &Arc<crate::agent::react::run::pipeline::ToolExecutionPipeline> =
                if let Some(ref p) = self.tool_execution_pipeline {
                    p
                } else {
                    default_pipeline = Arc::new(
                        crate::agent::react::run::pipeline::ToolExecutionPipeline::default_pipeline(
                        ),
                    );
                    &default_pipeline
                };
            self.execute_with_pipeline(tool_name, input, soften_errors, pipeline)
                .await
        })
    }

    /// Run the PostToolUseFailure hook and check for block.
    ///
    ///
    /// Returns `Err(ToolExecutionFailure)` if a hook blocks the error output.
    #[allow(dead_code)]
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

    /// Delegate to the run snapshot's authoritative spill/truncation policy.
    pub(crate) async fn truncate_tool_output(&self, output: String) -> String {
        crate::agent::snapshot::AgentRunSnapshot::from_agent(self)
            .truncate_tool_output(output)
            .await
    }

    /// Perform guard check on tool output to prevent malicious content injection
    ///
    /// If a guard manager is configured, output is checked for safety.
    /// Returns `Some(filtered_output)` if output was filtered/modified,
    /// returns `None` if output is fine and needs no modification.
    #[allow(dead_code)]
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
                if let Err(e) = al.log(event).await {
                    tracing::error!(error = %e, "audit log write failed — event dropped");
                }
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

        // Create a snapshot for the pipeline
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(self);

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
            stream_tx: None,
        };

        match pipeline.run(&mut ctx, &snapshot).await {
            Ok(()) => {
                if ctx.blocked {
                    self.record_memory_trigger_tool_event(
                        tool_name,
                        input,
                        None,
                        ctx.block_reason.as_deref(),
                    );
                    return Ok(ToolExecutionOutcome {
                        tool_result: None,
                        output: ctx
                            .block_reason
                            .unwrap_or_else(|| format!("Tool {} blocked", tool_name)),
                        hook_messages: ctx.hook_messages,
                    });
                }

                if let Some(result) = ctx.result {
                    self.record_memory_trigger_tool_event(tool_name, input, Some(&result), None);
                    if result.success {
                        self.record_skill_telemetry(tool_name, ctx.duration_ms, true, None);
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
                            self.record_skill_telemetry(
                                tool_name,
                                ctx.duration_ms,
                                false,
                                Some(&error_msg),
                            );
                            Ok(ToolExecutionOutcome {
                                tool_result: Some(result),
                                output: format!(
                                    "[Tool execution failed] {err}\nTip: adjust parameters based on the error and retry, or try other tools."
                                ),
                                hook_messages: ctx.hook_messages,
                            })
                        } else {
                            self.record_skill_telemetry(
                                tool_name,
                                ctx.duration_ms,
                                false,
                                Some(&error_msg),
                            );
                            Err(ToolExecutionFailure {
                                error: err,
                                hook_messages: ctx.hook_messages,
                            })
                        }
                    }
                } else {
                    self.record_memory_trigger_tool_event(
                        tool_name,
                        input,
                        None,
                        Some("Pipeline completed without result"),
                    );
                    Err(ToolExecutionFailure {
                        error: ReactError::Other("Pipeline completed without result".into()),
                        hook_messages: ctx.hook_messages,
                    })
                }
            }
            Err(error) => {
                self.record_memory_trigger_tool_event(
                    tool_name,
                    input,
                    None,
                    Some(&error.to_string()),
                );
                self.record_skill_telemetry(
                    tool_name,
                    ctx.duration_ms,
                    false,
                    Some(&error.to_string()),
                );
                Err(ToolExecutionFailure {
                    error,
                    hook_messages: ctx.hook_messages,
                })
            }
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
