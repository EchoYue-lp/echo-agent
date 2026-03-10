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
use crate::human_loop::{HumanLoopRequest, HumanLoopResponse};
use crate::llm::types::{FunctionCall, Message, ToolCall as LlmToolCall};
use crate::llm::{chat, stream_chat};
use crate::tools::ToolParameters;
use futures::StreamExt;
use futures::future::join_all;
use futures::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;
use tracing::{debug, info, warn};

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

        // 获取人工审批状态
        // 保守策略：读取失败时默认需要审批（安全优先）
        let needs_approval = {
            let approval_manager = match self.human_in_loop.read() {
                Ok(guard) => guard,
                Err(e) => {
                    tracing::error!(
                        "Human in loop lock poisoned, defaulting to require approval: {}",
                        e
                    );
                    return Err(crate::error::ReactError::Agent(
                        crate::error::AgentError::InitializationFailed(
                            "Human approval system unavailable".to_string(),
                        ),
                    )
                    .into());
                }
            };
            approval_manager.needs_approval(tool_name)
        };

        if needs_approval {
            warn!(agent = %agent, tool = %tool_name, "⚠️ 工具需要人工审批");
            let req = HumanLoopRequest::approval(tool_name, input.clone());
            match self.approval_provider.request(req).await? {
                HumanLoopResponse::Approved => {
                    info!(agent = %agent, tool = %tool_name, "✅ 用户批准执行工具");
                }
                HumanLoopResponse::Rejected { reason } => {
                    warn!(agent = %agent, tool = %tool_name, reason = ?reason, "❌ 用户拒绝执行工具");
                    return Ok(format!(
                        "用户已拒绝执行工具 {}{}",
                        tool_name,
                        reason.map(|r| format!("，原因：{r}")).unwrap_or_default()
                    ));
                }
                HumanLoopResponse::Timeout => {
                    warn!(agent = %agent, tool = %tool_name, "⏰ 审批超时，工具未执行");
                    return Ok(format!("工具 {tool_name} 审批超时，已跳过执行"));
                }
                HumanLoopResponse::Text(_) => {
                    warn!(agent = %agent, tool = %tool_name, "⚠️ 审批请求收到意外的 Text 响应，视为拒绝");
                    return Ok(format!("工具 {tool_name} 审批异常，已跳过执行"));
                }
            }
        }

        let result = self.tool_manager.execute_tool(tool_name, params).await?;

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

    /// 执行工具，并根据 `tool_error_feedback` 配置决定失败时的行为：
    /// - `true`（默认）：将错误信息转换为工具观测值回传给 LLM，让模型自行纠错
    /// - `false`：直接向上抛出 `Err`，与旧行为一致
    ///
    /// `final_answer` 工具始终保持原始错误语义，不会被软化。
    pub(crate) async fn execute_tool_feedback(
        &self,
        tool_name: &str,
        input: &Value,
    ) -> Result<String> {
        match self.execute_tool(tool_name, input).await {
            Ok(result) => Ok(result),
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

        let messages = self.context.prepare(None).await?;

        for cb in &callbacks {
            cb.on_think_start(&agent, &messages).await;
        }

        let tools = self.tool_manager.to_openai_tools();
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

        // 检查是否有需要人工审批的工具
        // 保守策略：读取失败时默认有审批工具（串行执行，更安全）
        let has_approval_tools = {
            let approval_manager = match self.human_in_loop.read() {
                Ok(guard) => guard,
                Err(e) => {
                    tracing::error!(
                        "Human in loop lock poisoned at tool check, defaulting to serial: {}",
                        e
                    );
                    // 回退为串行执行
                    return Ok(Some(format!(
                        "[系统错误：人工审批系统不可用，工具 {} 被跳过]",
                        tool_calls
                            .iter()
                            .map(|(_, n, _)| n.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
            };
            tool_calls
                .iter()
                .any(|(_, name, _)| approval_manager.needs_approval(name))
        };

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
                .map(|(_, name, args)| self.execute_tool_feedback(name, args))
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

        self.run_react_loop(task).await
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

        self.run_react_loop(message).await
    }

    /// 核心 ReAct 循环（注入记忆 → 追加消息 → think/act 迭代）。
    /// `run_direct` 和 `run_chat_direct` 共享此实现。
    async fn run_react_loop(&mut self, message: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();
        let callbacks = self.config.callbacks.clone();

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
            for cb in &callbacks {
                cb.on_iteration(&agent, iteration).await;
            }

            debug!(agent = %agent, iteration = iteration + 1, "--- 迭代 ---");

            let steps = self.think().await?;
            if steps.is_empty() {
                warn!(agent = %agent, "LLM 没有响应");
                return Err(ReactError::from(AgentError::NoResponse));
            }

            if let Some(answer) = self.process_steps(steps).await? {
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
        }

        warn!(agent = %agent, max = self.config.max_iterations, "达到最大迭代次数");
        Err(ReactError::from(AgentError::MaxIterationsExceeded(
            self.config.max_iterations,
        )))
    }

    // ── 流式执行公共方法 ─────────────────────────────────────────────────────────

    /// 流式执行的公共初始化逻辑
    ///
    /// 根据模式决定是否重置上下文、是否从 checkpoint 恢复
    pub(crate) async fn prepare_stream_context(&mut self, mode: StreamMode, input: &str) {
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
        if let Some(store) = &self.store {
            let agent_name = self.config.agent_name.clone();
            let ns = vec![agent_name.as_str(), "memories"];
            if let Ok(items) = store.semantic_search(&ns, input, 5).await
                && !items.is_empty()
            {
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
    }

    /// 流式执行的 LLM 请求（带重试）
    pub(crate) async fn create_llm_stream(
        &mut self,
        messages: Vec<Message>,
    ) -> Result<BoxStream<'static, Result<crate::llm::types::ChatCompletionChunk>>> {
        let agent = &self.config.agent_name;
        let tools_for_stream: Option<Vec<_>> = if self.config.enable_tool {
            let tools = self.tool_manager.to_openai_tools();
            if tools.is_empty() { None } else { Some(tools) }
        } else {
            None
        };

        let max_retries = self.config.llm_max_retries;
        let retry_delay = self.config.llm_retry_delay_ms;
        let client = self.client.clone();
        let model_name = self.config.model_name.clone();
        let response_format = self.config.response_format.clone();

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
                        if let Some(name) = &f.name {
                            if !name.is_empty() {
                                entry.1 = name.clone();
                            }
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
            self.prepare_stream_context(mode, &input).await;

            // 根据模式输出不同的日志
            match mode {
                StreamMode::Execute => info!(agent = %agent, "🌊 Agent 开始流式执行任务"),
                StreamMode::Chat => info!(agent = %agent, "🌊 Agent 开始流式多轮对话"),
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
