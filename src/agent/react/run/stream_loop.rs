//! Streaming execution loop

use super::super::{ReactAgent, StepType, TOOL_FINAL_ANSWER};
use super::execution::{ToolExecutionFailure, ToolExecutionOutcome};
use crate::agent::AgentEvent;
use crate::error::{AgentError, ReactError, Result, ToolError};
use crate::llm::stream_chat;
use crate::llm::types::{FunctionCall, Message, ToolCall as LlmToolCall};
use futures::StreamExt;
use futures::future::join_all;
use futures::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;
use tracing::{Instrument, debug, info, info_span};

/// Streaming execution mode configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamMode {
    /// Single-round execution mode: reset context, restore from checkpoint
    Execute,
    /// Multi-round conversation mode: preserve context, do not reset
    Chat,
}

impl std::fmt::Display for StreamMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamMode::Execute => f.write_str("execute"),
            StreamMode::Chat => f.write_str("chat"),
        }
    }
}

/// Streaming execution initialization parameters
pub(crate) struct StreamInit {
    /// User input text (for audit logs and memory recall)
    pub text: String,
    /// Optional pre-built Message (multimodal scenarios), auto-constructs text message when None
    pub message: Option<Message>,
    /// Log label (e.g. "" or "(multimodal)")
    pub label: String,
}

impl ReactAgent {
    /// Process streaming response chunk, collect content and return events
    ///
    /// `in_reasoning` tracks whether reasoning_content is being output (Qwen3/DeepSeek thinking process).
    /// ThinkStart is emitted when reasoning_content is first encountered,
    /// ThinkEnd is emitted when content or tool_calls is first encountered after reasoning ends.
    /// This way ThinkStart/ThinkEnd only wraps the thinking process, not the entire LLM response.
    #[allow(clippy::type_complexity)]
    pub(crate) fn process_stream_chunk(
        chunk: &crate::llm::types::ChatCompletionChunk,
        content_buffer: &mut String,
        tool_call_map: &mut HashMap<u32, (String, String, String)>,
        in_reasoning: &mut bool,
    ) -> Vec<AgentEvent> {
        let mut events = Vec::new();

        if let Some(choice) = chunk.choices.first() {
            // Handle reasoning_content (Qwen3/DeepSeek thinking process)
            if let Some(reasoning) = &choice.delta.reasoning_content
                && !reasoning.is_empty()
            {
                if !*in_reasoning {
                    *in_reasoning = true;
                    events.push(AgentEvent::ThinkStart);
                }
                // Note: reasoning is only displayed as thinking process, not mixed into content_buffer
                events.push(AgentEvent::Token(reasoning.clone()));
            }

            // When content is first encountered after reasoning ends, close the thinking block
            if let Some(content) = &choice.delta.content
                && !content.is_empty()
            {
                if *in_reasoning {
                    *in_reasoning = false;
                    events.push(AgentEvent::ThinkEnd {
                        prompt_tokens: 0,
                        completion_tokens: 0,
                    });
                }
                content_buffer.push_str(content);
                events.push(AgentEvent::Token(content.clone()));
            }

            if let Some(delta_calls) = &choice.delta.tool_calls {
                // When tool_calls is first encountered after reasoning ends, close the thinking block
                if *in_reasoning {
                    *in_reasoning = false;
                    events.push(AgentEvent::ThinkEnd {
                        prompt_tokens: 0,
                        completion_tokens: 0,
                    });
                }
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

        events
    }

    /// Convert the collected tool_call_map into structured tool call lists
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

    /// Create streaming LLM request (with retries)
    #[tracing::instrument(skip(self, messages), fields(agent = %self.config.agent_name, model = %self.config.model_name, msg_count = messages.len()))]
    pub(crate) async fn create_llm_stream(
        &self,
        messages: Vec<Message>,
    ) -> Result<BoxStream<'static, Result<crate::llm::types::ChatCompletionChunk>>> {
        let cancel_token = self.cancel_token.lock().await.clone();
        let agent = &self.config.agent_name;
        let tools_for_stream: Option<Vec<_>> = if self.config.enable_tool {
            let tools = self.tools.tool_manager.get_openai_tools();
            if tools.is_empty() { None } else { Some(tools) }
        } else {
            None
        };

        let max_retries = self.config.llm_max_retries;
        let retry_delay = self.config.llm_retry_delay_ms;
        let client = self.client.clone();
        let model_name = self.config.model_name.clone();
        let response_format = self.config.response_format.clone();
        let temperature = self.config.temperature;
        let max_tokens = self.config.max_tokens;

        info!(agent = %agent, model = %model_name, "📡 Creating LLM streaming request");

        let circuit_breaker = self.guard.circuit_breaker.clone();
        let stream_result =
            super::retry::retry_llm_call(agent, max_retries, retry_delay, &circuit_breaker, || {
                let client = client.clone();
                let model_name = model_name.clone();
                let messages = messages.clone();
                let tools_for_stream = tools_for_stream.clone();
                let response_format = response_format.clone();
                let cancel_token = cancel_token.clone();
                async move {
                    stream_chat(
                        client,
                        &model_name,
                        messages,
                        temperature,
                        max_tokens,
                        tools_for_stream,
                        None,
                        response_format,
                        cancel_token, // wired from chat_stream_with_cancel / execute_stream_with_cancel
                    )
                    .await
                }
            })
            .await;

        let stream = stream_result?;
        Ok(Box::pin(stream))
    }

    /// Unified entry point for streaming execution
    ///
    /// Decides based on `mode` parameter:
    /// - `StreamMode::Execute`: reset context, restore from checkpoint, suitable for single-round tasks
    /// - `StreamMode::Chat`: preserve context, suitable for multi-round conversations
    #[tracing::instrument(skip(self), fields(agent = %self.config.agent_name, model = %self.config.model_name, mode = %mode))]
    pub(crate) async fn run_stream(
        &self,
        input: &str,
        mode: StreamMode,
    ) -> Result<futures::stream::BoxStream<'_, Result<AgentEvent>>> {
        self.run_stream_inner(
            StreamInit {
                text: input.to_string(),
                message: None,
                label: String::new(),
            },
            mode,
        )
        .await
    }

    /// Unified entry point for streaming execution (multimodal message version)
    ///
    /// Same as `run_stream`, but accepts a pre-built `Message` instead of `&str`,
    /// supporting multimodal input with content parts such as images and files.
    /// Context reset behavior is decided by the `mode` parameter.
    #[tracing::instrument(skip(self), fields(agent = %self.config.agent_name, model = %self.config.model_name, mode = %mode))]
    pub(crate) async fn run_stream_with_message(
        &self,
        message: Message,
        mode: StreamMode,
    ) -> Result<futures::stream::BoxStream<'_, Result<AgentEvent>>> {
        let text = message.content.as_text().unwrap_or_default();
        self.run_stream_inner(
            StreamInit {
                text,
                message: Some(message),
                label: "(multimodal)".to_string(),
            },
            mode,
        )
        .await
    }

    #[tracing::instrument(skip(self, init), fields(agent = %self.config.agent_name, model = %self.config.model_name, mode = %mode))]
    async fn run_stream_inner(
        &self,
        init: StreamInit,
        mode: StreamMode,
    ) -> Result<futures::stream::BoxStream<'_, Result<AgentEvent>>> {
        // Prepare context outside try_stream! to avoid lifetime issues
        let recalled = if let Some(ref msg) = init.message {
            self.prepare_stream_context_with_message(mode, msg).await
        } else {
            self.prepare_stream_context(mode, &init.text).await
        };

        // Clone Arcs before entering try_stream! to avoid capturing &self
        let context = self.memory.context.clone();
        let text = init.text;
        let _message = init.message;
        let label = init.label;
        let stream = async_stream::try_stream! {
            let agent = self.config.agent_name.clone();
            let callbacks = self.config.callbacks.clone();

            // Log differently based on mode
            match mode {
                StreamMode::Execute => info!(agent = %agent, "🌊 Agent starting streaming task execution{label}"),
                StreamMode::Chat => info!(agent = %agent, "🌊 Agent starting streaming multi-round conversation{label}"),
            }

            if recalled > 0 {
                yield AgentEvent::MemoryRecalled { count: recalled };
            }

            self.log_user_input_audit(&text).await;

            for iteration in 0..self.config.max_iterations {
                for cb in &callbacks {
                    cb.on_iteration(&agent, iteration).await;
                }

                debug!(agent = %agent, iteration = iteration + 1, "--- Streaming iteration{label} ---");

                let prepare_result = context.lock().await.prepare(None).await?;

                if let Some(ref stats) = prepare_result.compressed {
                    yield AgentEvent::ContextCompressed {
                        before_count: stats.before_count,
                        after_count: stats.after_count,
                        before_tokens: stats.before_tokens,
                        after_tokens: stats.after_tokens,
                    };
                }

                let messages = prepare_result.messages;

                for cb in &callbacks {
                    cb.on_think_start(&agent, &messages).await;
                }

                // Create LLM stream
                let llm_stream = self.create_llm_stream(messages.clone()).await?;
                let mut llm_stream = Box::pin(llm_stream);

                // Collect streaming response
                let mut content_buffer = String::new();
                let mut tool_call_map: HashMap<u32, (String, String, String)> = HashMap::new();
                let mut last_usage: Option<crate::llm::types::Usage> = None;
                let mut in_reasoning = false;

                while let Some(chunk_result) = llm_stream.next().await {
                    let chunk = chunk_result?;
                    if chunk.usage.is_some() {
                        last_usage = chunk.usage.clone();
                    }
                    for event in Self::process_stream_chunk(&chunk, &mut content_buffer, &mut tool_call_map, &mut in_reasoning) {
                        yield event;
                    }
                }

                let prompt_tokens = last_usage
                    .as_ref()
                    .and_then(|u| u.prompt_tokens)
                    .unwrap_or(0) as usize;
                let completion_tokens = last_usage
                    .as_ref()
                    .and_then(|u| u.completion_tokens)
                    .unwrap_or(0) as usize;

                // If still in reasoning state after stream ends (model only output reasoning without content), close the thinking block
                if in_reasoning {
                    yield AgentEvent::ThinkEnd {
                        prompt_tokens,
                        completion_tokens,
                    };
                }

                // Check if there are tool calls
                let has_tool_calls = !tool_call_map.is_empty();

                if has_tool_calls {
                    // Build tool calls
                    let (msg_tool_calls, steps) = Self::build_tool_calls_from_map(&tool_call_map);

                    // Emit ToolCall events
                    for (_, name, args) in &steps {
                        yield AgentEvent::ToolCall {
                            name: name.clone(),
                            args: args.clone(),
                        };
                    }

                    // Trigger on_think_end callbacks
                    {
                        let think_steps: Vec<StepType> = steps.iter().map(|(id, name, args)| {
                            StepType::Call {
                                tool_call_id: id.clone(),
                                function_name: name.clone(),
                                arguments: args.clone(),
                            }
                        }).collect();
                        for cb in &callbacks {
                            cb.on_think_end(&agent, &think_steps, prompt_tokens, completion_tokens).await;
                        }
                    }

                    // Push assistant message to context
                    context.lock().await.push(Message::assistant_with_tools(msg_tool_calls));

                    // Separate approval tools and concurrent tools
                    // Approval tools are forced to execute serially, non-approval tools execute concurrently
                    #[cfg(feature = "human-loop")]
                    let (approval_steps, concurrent_steps) = {
                        let mut approval = Vec::new();
                        let mut concurrent = Vec::new();
                        for step in steps {
                            if self.tool_needs_approval(&step.1).await {
                                approval.push(step);
                            } else {
                                concurrent.push(step);
                            }
                        }
                        (approval, concurrent)
                    };
                    #[cfg(not(feature = "human-loop"))]
                    let (approval_steps, concurrent_steps): (
                        Vec<(String, String, Value)>,
                        Vec<(String, String, Value)>,
                    ) = (Vec::new(), steps);

                    // Execute non-approval tools concurrently
                    if !concurrent_steps.is_empty() {
                        let max_concurrency = self.tools.tool_manager.max_concurrency();
                        let concurrent_len = concurrent_steps.len();
                        let tool_names: Vec<&str> =
                            concurrent_steps.iter().map(|(_, n, _)| n.as_str()).collect();
                        info!(
                            agent = %agent,
                            tools = ?tool_names,
                            max_concurrency = ?max_concurrency,
                            "⚡ Streaming concurrent execution of {} tool calls",
                            concurrent_len,
                        );

                        let futures: Vec<_> = concurrent_steps
                            .iter()
                            .map(|(_, name, args)| {
                                self.execute_tool_feedback_raw(
                                    name,
                                    args,
                                    self.config.tool_error_feedback,
                                )
                                .instrument(info_span!("tool_execute", tool.name = %name))
                            })
                            .collect();

                        let batch_timeout = super::retry::compute_concurrent_tool_batch_timeout(
                            &self.config.tool_execution,
                            futures.len(),
                            max_concurrency,
                        );

                        let results: Vec<
                            std::result::Result<ToolExecutionOutcome, ToolExecutionFailure>,
                        >;
                        if let Some(timeout) = batch_timeout {
                            results = tokio::time::timeout(timeout, join_all(futures)).await
                                .map_err(|_| {
                                    ReactError::from(ToolError::Timeout(format!(
                                        "parallel tool batch exceeded total timeout after {:?}",
                                        timeout
                                    )))
                                })?;
                        } else {
                            results = join_all(futures).await;
                        }

                        for (step, result) in
                            concurrent_steps.into_iter().zip(results)
                        {
                            let tool_call_id = step.0;
                            let function_name = step.1;
                            let tool_result = match result {
                                Ok(outcome) => {
                                    self.apply_hook_messages(
                                        &function_name,
                                        &outcome.hook_messages,
                                    )
                                    .await;
                                    Ok(self.truncate_tool_output(outcome.output).await)
                                }
                                Err(failure) => {
                                    self.apply_hook_messages(
                                        &function_name,
                                        &failure.hook_messages,
                                    )
                                    .await;
                                    Err(failure.error)
                                }
                            };

                            match tool_result {
                                Ok(output) => {
                                    yield AgentEvent::ToolResult {
                                        name: function_name.clone(),
                                        output: output.clone(),
                                    };

                                    // Detect vega-lite chart output
                                    #[cfg(feature = "chart")]
                                    if output.contains("vega-lite")
                                        && let Ok(spec) =
                                            serde_json::from_str::<serde_json::Value>(&output)
                                    {
                                        yield AgentEvent::Chart { spec };
                                    }

                                    context.lock().await.push(Message::tool_result(
                                        tool_call_id,
                                        function_name.clone(),
                                        output.clone(),
                                    ));

                                    if function_name == TOOL_FINAL_ANSWER {
                                        self.auto_snapshot(iteration).await;
                                        for cb in &callbacks {
                                            cb.on_final_answer(&agent, &output).await;
                                        }
                                        info!(agent = %agent, "🏁 Streaming execution completed{label}");

                                        self.log_final_answer_audit(&output).await;
                                        self.save_checkpoint().await;

                                        yield AgentEvent::FinalAnswer(output);
                                        break;
                                    }
                                }
                                Err(error) => {
                                    yield AgentEvent::ToolError {
                                        name: function_name.clone(),
                                        error: error.to_string(),
                                    };

                                    context.lock().await.push(Message::tool_result(
                                        tool_call_id,
                                        function_name.clone(),
                                        format!("[Error] {error}"),
                                    ));
                                }
                            }
                        }

                    }

                    // Execute approval tools serially
                    for (tool_call_id, function_name, arguments) in approval_steps {
                        match self
                            .execute_tool_feedback(&function_name, &arguments)
                            .await
                        {
                            Ok(output) => {
                                yield AgentEvent::ToolResult {
                                    name: function_name.clone(),
                                    output: output.clone(),
                                };

                                // Detect vega-lite chart output
                                #[cfg(feature = "chart")]
                                if output.contains("vega-lite")
                                    && let Ok(spec) =
                                        serde_json::from_str::<serde_json::Value>(&output)
                                {
                                    yield AgentEvent::Chart { spec };
                                }

                                context.lock().await.push(Message::tool_result(
                                    tool_call_id,
                                    function_name.clone(),
                                    output.clone(),
                                ));

                                if function_name == TOOL_FINAL_ANSWER {
                                    self.auto_snapshot(iteration).await;
                                    for cb in &callbacks {
                                        cb.on_final_answer(&agent, &output).await;
                                    }
                                    info!(agent = %agent, "🏁 Streaming execution completed{label}");

                                    self.log_final_answer_audit(&output).await;
                                    self.save_checkpoint().await;

                                    yield AgentEvent::FinalAnswer(output);
                                    return;
                                }
                            }
                            Err(error) => {
                                yield AgentEvent::ToolError {
                                    name: function_name.clone(),
                                    error: error.to_string(),
                                };

                                context.lock().await.push(Message::tool_result(
                                    tool_call_id,
                                    function_name.clone(),
                                    format!("[Error] {error}"),
                                ));
                            }
                        }
                    }

                    self.auto_snapshot(iteration).await;
                } else if !content_buffer.is_empty() {
                    // Plain text response
                    let think_steps = vec![StepType::Thought(content_buffer.clone())];
                    for cb in &callbacks {
                        cb.on_think_end(&agent, &think_steps, prompt_tokens, completion_tokens).await;
                    }
                    for cb in &callbacks {
                        cb.on_final_answer(&agent, &content_buffer).await;
                    }
                    context.lock().await.push(Message::assistant(content_buffer.clone()));

                    self.auto_snapshot(iteration).await;
                    self.log_final_answer_audit(&content_buffer).await;
                    self.save_checkpoint().await;

                    yield AgentEvent::FinalAnswer(content_buffer);
                    return;
                } else {
                    Err(ReactError::Agent(AgentError::NoResponse {
                        model: self.config.model_name.clone(),
                        agent: self.config.agent_name.clone(),
                    }))?;
                }
            }

            Err(ReactError::Agent(AgentError::MaxIterationsExceeded(
                self.config.max_iterations,
            )))?;
        };

        Ok(Box::pin(stream))
    }
}
