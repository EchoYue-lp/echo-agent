use crate::compression::compressor::SlidingWindowCompressor;
use crate::compression::compressor::sliding_window::{message_tokens, select_recent_tail};
use crate::compression::{
    CompressionCheckpoint, CompressionInput, CompressionOutput, ContextCompressor,
    StructuredSummary,
};
use echo_core::error::Result;
use echo_core::llm::LlmClient;
use echo_core::llm::types::{ContentPart, Message, MessageContent, ResponseFormat, Role};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::warn;

/// Type alias for summary prompt builder closures
pub type SummaryPromptFn = Box<dyn Fn(&[Message]) -> String + Send + Sync>;

const COMPRESSION_PROMPT: &str =
    "你的任务是生成一个可供后续 Agent 继续工作的语义检查点，而不是转述整段对话。
摘要会与近期原始消息、系统/项目规则和外部文件一起重新注入模型。

必须保留：
1. 用户的当前目标、最新意图和验收条件。
2. 明确约束、用户偏好、禁止事项与已做决策及其原因。
3. 已完成、正在进行和待处理的工作，不得把计划写成已完成。
4. 继续工作所需的精确文件路径、标识符、版本、数值、命令、测试结果和错误。
5. 工具输出的关键结论与证据位置；不复制可以从仓库或 transcript 重新读取的大段正文。
6. 紧接着应该执行的下一步。

必须自包含、准确、紧凑。不要依赖被压缩掉的原文，不要编造进度或结果。";

/// 使用内置中文模板生成默认摘要提示词。
///
/// 公共自由函数，供实现自定义 [`ContextCompressor`] 的用户复用内置模板。
/// 如果你只是想用默认摘要策略，直接构造 [`SummaryCompressor::new`] 即可，无需调用此函数。
///
/// # 示例
///
/// ```rust
/// use echo_core::llm::types::Message;
/// use echo_state::compression::compressor::default_summary_prompt;
///
/// let messages = vec![Message::user("你好".to_string()), Message::assistant("你好！".to_string())];
/// let prompt = default_summary_prompt(&messages);
/// ```
pub fn default_summary_prompt(messages: &[Message]) -> String {
    default_summary_prompt_with_focus(messages, None)
}

/// Build a summary prompt with optional user-provided focus instructions.
///
/// When `focus` is provided, it is injected as a high-priority instruction
/// asking the LLM to pay special attention to the specified topics.
pub fn default_summary_prompt_with_focus(messages: &[Message], focus: Option<&str>) -> String {
    let history = messages
        .iter()
        .map(message_for_summary)
        .collect::<Vec<_>>()
        .join("\n");

    let focus_instruction = focus
        .map(|f| {
            format!(
                "\n【重要】用户特别要求在摘要中重点关注以下内容，请确保这些信息在摘要中得到充分体现：\n{}\n",
                f
            )
        })
        .unwrap_or_default();

    format!(
        "请将以下对话历史压缩为简洁的摘要。\
        要求：\n {}。\
        {}\
        \n{}\n\n。",
        COMPRESSION_PROMPT, focus_instruction, history
    )
}

/// Structured summary prompt — asks the LLM to return JSON.
const STRUCTURED_SUMMARY_PROMPT: &str = r#"你的任务是创建对话历史的**结构化摘要**。你必须返回一个有效的 JSON 对象，包含以下字段：

{
  "goal": "用户的主要目标和意图（字符串）",
  "current_task": "当前正在执行的具体任务（字符串）",
  "completed_actions": ["已完成的具体行动1", "已完成的具体行动2"],
  "pending_tasks": ["待处理的具体任务1"],
  "decisions": ["已做出的决策：决策内容和原因"],
  "files_touched": ["涉及的文件路径，如 src/auth.rs"],
  "errors": ["遇到的错误及修复方法，格式：错误描述 → 修复方式"],
  "tool_outputs_summary": "工具输出中的关键发现摘要（字符串）",
  "user_preferences": ["用户表达的偏好，如 使用 pnpm"],
  "constraints": ["必须遵守的约束、禁止事项或验收条件"],
  "key_facts": ["继续工作必需的精确 ID、值、版本、命令、路径或证据位置"],
  "next_step": "建议的下一步行动（字符串）"
}

要求：
1. 只返回 JSON，不要有其他文字
2. 所有数组字段如果没有内容，使用空数组 []
3. 所有字符串字段如果没有内容，使用空字符串 ""
4. 摘要应足够详细，使另一个 AI 助手能无缝继续工作
5. 文件路径、标识符、数值、版本和命令必须保留精确形式
6. 不复制可从仓库或 transcript 按需取回的大段工具输出"#;

/// Build a structured-summary prompt, optionally with focus instructions.
pub fn structured_summary_prompt(messages: &[Message], focus: Option<&str>) -> String {
    let history = messages
        .iter()
        .map(message_for_summary)
        .collect::<Vec<_>>()
        .join("\n");

    let focus_instruction = focus
        .map(|f| format!("\n【重要】用户特别要求在摘要中重点关注以下内容：\n{}\n", f))
        .unwrap_or_default();

    format!(
        "{}\n{}\n\n对话历史：\n{}\n\n请返回 JSON：",
        STRUCTURED_SUMMARY_PROMPT, focus_instruction, history
    )
}

fn message_for_summary(message: &Message) -> String {
    let content = match &message.content {
        MessageContent::Text(text) => text.clone(),
        MessageContent::Empty => "[empty message]".to_string(),
        MessageContent::Parts(parts) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => text.clone(),
                ContentPart::ImageUrl { image_url } => {
                    let hash = echo_core::utils::hash::fnv1a_64(image_url.url.as_bytes());
                    if image_url.url.starts_with("data:") {
                        format!(
                            "[attachment:image content_hash={hash:016x} chars={}]",
                            image_url.url.chars().count()
                        )
                    } else {
                        format!(
                            "[attachment:image url={} content_hash={hash:016x}]",
                            image_url.url
                        )
                    }
                }
                ContentPart::File { name, content } => format!(
                    "[attachment:file name={} content_hash={:016x} chars={}]",
                    name,
                    echo_core::utils::hash::fnv1a_64(content.as_bytes()),
                    content.chars().count()
                ),
                ContentPart::ResourceLink { resource } => resource.model_text(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    format!("[{}]: {content}", message.role.as_str())
}

/// LLM 摘要压缩策略。
///
/// 将较早的对话历史发送给 LLM 生成摘要，摘要作为一条 `[对话历史摘要]` system 消息插入，
/// 最近 `keep_recent` 条消息保持原样不动。
///
/// 压缩后的消息结构：
/// ```text
/// [原有 system 消息]
/// [system] [对话历史摘要] <-- 新插入
/// [最近 keep_recent 条对话消息]
/// ```
///
/// **失败回退**：当 LLM 调用失败（超时、API 错误等），自动回退到
/// [`SlidingWindowCompressor`]（保留最近 `keep_recent` 条）。
///
/// # 构造方式
///
/// - [`SummaryCompressor::new`] — 使用内置中文摘要模板
/// - [`SummaryCompressor::with_prompt`] — 使用自定义 prompt 闭包
///
/// # 完全自定义
///
/// 如果你想修改压缩逻辑本身（增量摘要、不同的回退策略、摘要不放入 system 消息等），
/// 请直接实现 [`ContextCompressor`]。你可以在自己的实现中调用
/// [`default_summary_prompt()`] 复用内置模板。
///
/// 适用场景：
/// - 长线任务规划（将已完成步骤压缩为状态摘要）
/// - 需要记住角色设定和重大事件，但不需要保留全部细节
pub struct SummaryCompressor {
    llm: Arc<dyn LlmClient>,
    prompt_fn: SummaryPromptFn,
    /// 最近多少条对话消息保持原样（不参与摘要）
    keep_recent: usize,
    recent_token_budget: Option<usize>,
}

impl SummaryCompressor {
    /// 使用内置中文摘要模板构造。
    pub fn new(llm: Arc<dyn LlmClient>, keep_recent: usize) -> Self {
        Self {
            llm,
            prompt_fn: Box::new(default_summary_prompt),
            keep_recent,
            recent_token_budget: None,
        }
    }

    /// 使用自定义 prompt 闭包构造。
    ///
    /// 闭包接收待摘要的消息切片，返回发给 LLM 的 prompt 字符串。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_state::compression::compressor::SummaryCompressor;
    /// use echo_core::llm::LlmClient;
    /// use std::sync::Arc;
    ///
    /// # async fn example(llm: Arc<dyn LlmClient>) {
    /// let compressor = SummaryCompressor::with_prompt(
    ///     llm,
    ///     6,
    ///     |messages| format!("用英文总结以下 {} 条对话的核心结论", messages.len()),
    /// );
    /// # }
    /// ```
    pub fn with_prompt(
        llm: Arc<dyn LlmClient>,
        keep_recent: usize,
        prompt_fn: impl Fn(&[Message]) -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            llm,
            prompt_fn: Box::new(prompt_fn),
            keep_recent,
            recent_token_budget: None,
        }
    }

    /// Use a token allowance for recent turns instead of the legacy message cap.
    pub fn with_recent_token_budget(mut self, tokens: usize) -> Self {
        self.recent_token_budget = (tokens > 0).then_some(tokens);
        self
    }
}

impl SummaryCompressor {
    /// Try structured JSON summary; fall back to natural language on failure.
    ///
    /// Returns `(summary_text, optional_structured)`.
    /// - If structured output succeeded: `(Some(json_text), Some(structured_summary))`
    /// - If natural language fallback: `(Some(text), None)`
    /// - If both failed: `(None, None)` → caller should fall back to SlidingWindow
    async fn try_structured_summary(
        &self,
        messages: &[Message],
        focus: Option<&str>,
        cancel_token: Option<echo_core::compression::CancellationToken>,
    ) -> (Option<String>, Option<StructuredSummary>) {
        // Check if provider supports structured output
        let supports_structured = self.llm.capabilities().structured_output;

        if supports_structured {
            // Path A: Use ResponseFormat::JsonSchema for reliable JSON output
            let prompt = structured_summary_prompt(messages, focus);
            match self
                .llm
                .chat(echo_core::llm::ChatRequest {
                    messages: vec![Message::user(prompt)],
                    temperature: Some(0.3),
                    max_tokens: Some(2048),
                    tools: None,
                    tool_choice: None,
                    response_format: Some(ResponseFormat::JsonObject),
                    thinking: None,
                    cancel_token: cancel_token.clone(),
                    timeouts: None,
                    user_id: None,
                    cache_hints: None,
                })
                .await
            {
                Ok(response) => {
                    let text = response.content().unwrap_or_default().to_string();
                    if text.trim().is_empty() {
                        warn!("Structured summary returned empty content");
                    } else if let Some(parsed) = StructuredSummary::from_llm_response(&text) {
                        return (Some(text), Some(parsed));
                    } else {
                        warn!("Structured summary JSON parse failed, using raw text");
                        return (Some(text), None);
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Structured summary LLM call failed, falling back to natural language");
                    // Fall through to natural language below
                }
            }
        }

        // Path B: Natural language (no structured output support or structured failed)
        let base_prompt = (self.prompt_fn)(messages);
        let prompt = if let Some(f) = focus {
            format!("{}\n\n【重要】用户特别要求重点关注：{}", base_prompt, f)
        } else {
            base_prompt
        };

        match self
            .llm
            .chat(echo_core::llm::ChatRequest {
                messages: vec![Message::user(prompt)],
                temperature: None,
                max_tokens: None,
                tools: None,
                tool_choice: None,
                response_format: None,
                thinking: None,
                cancel_token,
                timeouts: None,
                user_id: None,
                cache_hints: None,
            })
            .await
        {
            Ok(response) => response
                .content()
                .filter(|text| !text.trim().is_empty())
                .map(|text| (Some(text.to_string()), None))
                .unwrap_or((None, None)),
            Err(_) => (None, None),
        }
    }
}

impl ContextCompressor for SummaryCompressor {
    fn name(&self) -> &str {
        "Summary"
    }

    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        Box::pin(async move {
            check_cancelled(&input)?;
            let start = Instant::now();
            let tokenizer = input.tokenizer();
            let focus = input
                .focus_instructions
                .clone()
                .or(input.current_query.clone());
            let tokens_before: usize = input
                .messages
                .iter()
                .filter_map(|m| m.content.as_text())
                .map(|c| tokenizer.count_tokens(&c))
                .sum();

            let system_msgs: Vec<Message> = input
                .messages
                .iter()
                .filter(|message| message.role == Role::System && !is_generated_summary(message))
                .cloned()
                .collect();
            let conv_msgs: Vec<Message> = input
                .messages
                .iter()
                .filter(|message| message.role != Role::System || is_generated_summary(message))
                .cloned()
                .collect();
            let (to_summarize, to_keep) = select_recent_tail(
                conv_msgs,
                input
                    .token_limit
                    .saturating_sub(message_tokens(&system_msgs, tokenizer.as_ref())),
                self.recent_token_budget,
                self.keep_recent,
                true,
                tokenizer.as_ref(),
            )?;

            if to_summarize.is_empty() {
                let mut messages = system_msgs;
                messages.extend(to_keep);
                let tokens_after: usize = messages
                    .iter()
                    .filter_map(|m| m.content.as_text())
                    .map(|c| tokenizer.count_tokens(&c))
                    .sum();
                let checkpoint = CompressionCheckpoint::new(self.name())
                    .with_counts(messages.len(), 0)
                    .with_tokens(tokens_before, tokens_after)
                    .with_duration_ms(start.elapsed().as_millis() as u64)
                    .with_focus(focus.clone());
                return Ok(CompressionOutput {
                    messages,
                    evicted: vec![],
                    checkpoint: Some(checkpoint),
                });
            }

            // Try structured output first; fall back to natural language
            let (summary_text, structured) = self
                .try_structured_summary(&to_summarize, focus.as_deref(), input.cancel_token.clone())
                .await;
            check_cancelled(&input)?;

            let (final_summary, summary_for_checkpoint) = match (summary_text, structured) {
                (Some(_text), Some(ref s)) => {
                    // Structured summary succeeded — store as JSON system message
                    (s.to_system_message(), Some(s.to_json()))
                }
                (Some(text), None) => {
                    // Natural language fallback
                    (format!("[对话历史摘要]\n{}", text), Some(text))
                }
                (None, _) => {
                    // LLM call itself failed — fall back to sliding window
                    warn!("⚠️ LLM 摘要生成失败，回退到滑动窗口压缩");
                    return SlidingWindowCompressor::new(self.keep_recent)
                        .with_recent_token_budget(
                            self.recent_token_budget.unwrap_or(input.token_limit),
                        )
                        .compress(input)
                        .await;
                }
            };

            let mut provisional = system_msgs;
            provisional.push(Message::system(final_summary));
            provisional.extend(to_keep);
            let bounded_result = SlidingWindowCompressor::new(self.keep_recent)
                .with_recent_token_budget(input.token_limit)
                .compress(CompressionInput {
                    messages: provisional,
                    token_limit: input.token_limit,
                    current_query: input.current_query.clone(),
                    focus_instructions: input.focus_instructions.clone(),
                    cancel_token: input.cancel_token.clone(),
                    tokenizer: Some(tokenizer.clone()),
                })
                .await;
            let bounded = match bounded_result {
                Ok(value) => value,
                Err(_) => {
                    return SlidingWindowCompressor::new(self.keep_recent)
                        .with_recent_token_budget(
                            self.recent_token_budget.unwrap_or(input.token_limit),
                        )
                        .compress(input)
                        .await;
                }
            };
            let messages = bounded.messages;
            // The candidate summary is first in the bounded conversation. If
            // the oldest prefix is evicted, exclude this newly generated item
            // from historical eviction/promotion and checkpoint provenance.
            let candidate_evicted = bounded.evicted.first().is_some_and(is_generated_summary);
            let mut evicted = to_summarize;
            evicted.extend(
                bounded
                    .evicted
                    .into_iter()
                    .skip(usize::from(candidate_evicted)),
            );

            let tokens_after: usize = messages
                .iter()
                .filter_map(|m| m.content.as_text())
                .map(|c| tokenizer.count_tokens(&c))
                .sum();

            let mut checkpoint = CompressionCheckpoint::new(self.name())
                .with_counts(messages.len(), evicted.len())
                .with_tokens(tokens_before, tokens_after)
                .with_duration_ms(start.elapsed().as_millis() as u64)
                .with_focus(focus);
            if !candidate_evicted {
                checkpoint = checkpoint.with_summary(summary_for_checkpoint.unwrap_or_default());
            }

            Ok(CompressionOutput {
                messages,
                evicted,
                checkpoint: Some(checkpoint),
            })
        })
    }
}

pub(crate) fn is_generated_summary(message: &Message) -> bool {
    message
        .content
        .as_text_ref()
        .is_some_and(|text| text.starts_with("[对话历史摘要]"))
}

fn check_cancelled(input: &CompressionInput) -> Result<()> {
    if input
        .cancel_token
        .as_ref()
        .is_some_and(|cancel| cancel.is_cancelled())
    {
        return Err(
            echo_core::error::AgentError::Cancelled("context compression".to_string()).into(),
        );
    }
    Ok(())
}

// ── Incremental Summary ───────────────────────────────────────────────────────

const INCREMENTAL_SUMMARY_PROMPT: &str = "You are maintaining a running summary of a conversation. Below you will find:\n\
     1. The PREVIOUS SUMMARY generated from earlier messages.\n\
     2. NEW MESSAGES that arrived since the last summary.\n\n\
     Please produce an UPDATED SUMMARY that incorporates the previous summary \
     and the new information. The updated summary should be self-contained — \
     another AI assistant reading it should be able to continue the conversation \
     without any other context.\n\n\
     Keep the same structure and level of detail as the previous summary.";

/// Incremental LLM summary compressor.
///
/// Unlike [`SummaryCompressor`] which re-summarizes ALL old messages every time,
/// `IncrementalSummaryCompressor` maintains the previous summary and only sends
/// the previous summary + new messages to the LLM. This reduces LLM cost and
/// latency for long conversations where compression triggers multiple times.
///
/// **How it works:**
/// 1. First compression: behaves like `SummaryCompressor` (summarizes all old messages)
/// 2. Subsequent compressions: sends `[previous summary] + [new messages since last summary]`
///    to the LLM, asking it to produce an updated summary
///
/// **Failure fallback**: Same as `SummaryCompressor` — falls back to `SlidingWindowCompressor`
/// on LLM errors.
///
/// # Example
///
/// ```rust,no_run
/// use echo_state::compression::compressor::IncrementalSummaryCompressor;
/// use echo_core::llm::LlmClient;
/// use std::sync::Arc;
///
/// # async fn example(llm: Arc<dyn LlmClient>) {
/// let compressor = IncrementalSummaryCompressor::new(llm, 6);
/// # }
/// ```
pub struct IncrementalSummaryCompressor {
    llm: Arc<dyn LlmClient>,
    keep_recent: usize,
    recent_token_budget: Option<usize>,
    /// Observation of the summary accepted by the context owner. Input messages
    /// remain the recovery authority; rejected calculations do not alter this cache.
    previous_summary: Mutex<Option<StructuredSummary>>,
}

impl IncrementalSummaryCompressor {
    pub fn new(llm: Arc<dyn LlmClient>, keep_recent: usize) -> Self {
        Self {
            llm,
            keep_recent,
            recent_token_budget: None,
            previous_summary: Mutex::new(None),
        }
    }

    /// Use the same token-based recent-turn selection as SummaryCompressor.
    pub fn with_recent_token_budget(mut self, tokens: usize) -> Self {
        self.recent_token_budget = (tokens > 0).then_some(tokens);
        self
    }

    /// Get the current stored summary as a JSON string (for backward compat).
    pub fn current_summary(&self) -> Option<String> {
        self.previous_summary
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|s| s.to_json()))
    }

    /// Get the current structured summary.
    pub fn current_structured_summary(&self) -> Option<StructuredSummary> {
        self.previous_summary.lock().ok().and_then(|g| g.clone())
    }

    /// Reset the stored summary.
    pub fn reset(&self) {
        if let Ok(mut guard) = self.previous_summary.lock() {
            *guard = None;
        }
    }
}

impl IncrementalSummaryCompressor {
    /// First compression: try structured output, fall back to natural language.
    async fn try_first_structured_summary(
        &self,
        messages: &[Message],
        focus: Option<&str>,
        cancel_token: Option<echo_core::compression::CancellationToken>,
    ) -> (Option<String>, Option<StructuredSummary>) {
        let supports_structured = self.llm.capabilities().structured_output;

        if supports_structured {
            let prompt = structured_summary_prompt(messages, focus);
            match self
                .llm
                .chat(echo_core::llm::ChatRequest {
                    messages: vec![Message::user(prompt)],
                    temperature: Some(0.3),
                    max_tokens: Some(2048),
                    tools: None,
                    tool_choice: None,
                    response_format: Some(ResponseFormat::JsonObject),
                    thinking: None,
                    cancel_token: cancel_token.clone(),
                    timeouts: None,
                    user_id: None,
                    cache_hints: None,
                })
                .await
            {
                Ok(response) => {
                    let text = response.content().unwrap_or_default().to_string();
                    if let Some(parsed) = StructuredSummary::from_llm_response(&text) {
                        return (Some(text), Some(parsed));
                    }
                    warn!("Incremental: structured parse failed, using raw text");
                    return (Some(text), None);
                }
                Err(e) => {
                    warn!(error = %e, "Incremental: structured LLM failed, fallback to natural language");
                }
            }
        }

        // Natural language fallback
        let prompt = default_summary_prompt_with_focus(messages, focus);
        match self
            .llm
            .chat(echo_core::llm::ChatRequest {
                messages: vec![Message::user(prompt)],
                cancel_token,
                ..Default::default()
            })
            .await
        {
            Ok(response) => (
                response.content().filter(|text| !text.trim().is_empty()),
                None,
            ),
            Err(_) => (None, None),
        }
    }

    /// Incremental: summarize new messages, then merge with previous summary field-by-field.
    async fn incremental_structured_summary(
        &self,
        new_messages: &[Message],
        previous: &StructuredSummary,
        focus: Option<&str>,
        cancel_token: Option<echo_core::compression::CancellationToken>,
    ) -> (Option<String>, Option<StructuredSummary>) {
        let supports_structured = self.llm.capabilities().structured_output;

        if supports_structured {
            let new_history = new_messages
                .iter()
                .filter_map(|m| {
                    m.content
                        .as_text()
                        .map(|c| format!("[{}]: {}", m.role.as_str(), c))
                })
                .collect::<Vec<_>>()
                .join("\n");

            let focus_note = focus
                .map(|f| format!("\nIMPORTANT: Focus on: {}\n", f))
                .unwrap_or_default();

            let prompt = format!(
                "{}\nPrevious summary (JSON):\n{}\n\nNew messages:\n{}\n\nReturn an updated JSON with the SAME structure.",
                STRUCTURED_SUMMARY_PROMPT,
                previous.to_json(),
                new_history
            );

            match self
                .llm
                .chat(echo_core::llm::ChatRequest {
                    messages: vec![Message::user(format!("{}{}", focus_note, prompt))],
                    temperature: Some(0.3),
                    max_tokens: Some(2048),
                    tools: None,
                    tool_choice: None,
                    response_format: Some(ResponseFormat::JsonObject),
                    thinking: None,
                    cancel_token: cancel_token.clone(),
                    timeouts: None,
                    user_id: None,
                    cache_hints: None,
                })
                .await
            {
                Ok(response) => {
                    let text = response.content().unwrap_or_default().to_string();
                    if let Some(parsed) = StructuredSummary::from_llm_response(&text) {
                        // Merge: previous + new (field-level)
                        let mut merged = previous.clone();
                        merged.merge_with(&parsed);
                        return (Some(text), Some(merged));
                    }
                    warn!("Incremental: structured merge parse failed");
                    return (Some(text), None);
                }
                Err(e) => {
                    warn!(error = %e, "Incremental: structured LLM failed");
                }
            }
        }

        // Natural language fallback for incremental
        let prev_json = previous.to_json();
        let new_history = new_messages
            .iter()
            .filter_map(|m| {
                m.content
                    .as_text()
                    .map(|c| format!("[{}]: {}", m.role.as_str(), c))
            })
            .collect::<Vec<_>>()
            .join("\n");

        let focus_note = focus
            .map(|f| format!("\nIMPORTANT: Focus on: {}\n", f))
            .unwrap_or_default();

        let prompt = format!(
            "{}{}\n\n--- PREVIOUS SUMMARY (JSON) ---\n{}\n\n--- NEW MESSAGES ---\n{}\n\n--- END ---\n\nProduce an updated summary (text format).",
            INCREMENTAL_SUMMARY_PROMPT, focus_note, prev_json, new_history
        );

        match self
            .llm
            .chat(echo_core::llm::ChatRequest {
                messages: vec![Message::user(prompt)],
                cancel_token,
                ..Default::default()
            })
            .await
        {
            Ok(response) => (
                response.content().filter(|text| !text.trim().is_empty()),
                None,
            ),
            Err(_) => (None, None),
        }
    }
}

impl ContextCompressor for IncrementalSummaryCompressor {
    fn context_committed(&self, messages: &[Message]) {
        let accepted = messages
            .iter()
            .filter(|message| is_generated_summary(message))
            .filter_map(|message| message.content.as_text_ref())
            .filter_map(|text| text.strip_prefix("[对话历史摘要]\n"))
            .find_map(StructuredSummary::from_llm_response);
        if let Ok(mut cache) = self.previous_summary.lock() {
            *cache = accepted;
        }
    }
    fn name(&self) -> &str {
        "IncrementalSummary"
    }

    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        Box::pin(async move {
            check_cancelled(&input)?;
            let start = Instant::now();
            let tokenizer = input.tokenizer();
            let focus = input
                .focus_instructions
                .clone()
                .or(input.current_query.clone());
            let tokens_before: usize = input
                .messages
                .iter()
                .filter_map(|m| m.content.as_text())
                .map(|c| tokenizer.count_tokens(&c))
                .sum();

            let system_msgs: Vec<Message> = input
                .messages
                .iter()
                .filter(|message| message.role == Role::System && !is_generated_summary(message))
                .cloned()
                .collect();
            let conv_msgs: Vec<Message> = input
                .messages
                .iter()
                .filter(|message| message.role != Role::System || is_generated_summary(message))
                .cloned()
                .collect();
            let (to_summarize, to_keep) = select_recent_tail(
                conv_msgs,
                input
                    .token_limit
                    .saturating_sub(message_tokens(&system_msgs, tokenizer.as_ref())),
                self.recent_token_budget,
                self.keep_recent,
                true,
                tokenizer.as_ref(),
            )?;

            if to_summarize.is_empty() {
                let mut messages = system_msgs;
                messages.extend(to_keep);
                let tokens_after: usize = messages
                    .iter()
                    .filter_map(|m| m.content.as_text())
                    .map(|c| tokenizer.count_tokens(&c))
                    .sum();
                let checkpoint = CompressionCheckpoint::new(self.name())
                    .with_counts(messages.len(), 0)
                    .with_tokens(tokens_before, tokens_after)
                    .with_duration_ms(start.elapsed().as_millis() as u64)
                    .with_focus(focus.clone());
                return Ok(CompressionOutput {
                    messages,
                    evicted: vec![],
                    checkpoint: Some(checkpoint),
                });
            }

            // Accepted input, rather than a prior calculation's private cache,
            // is the sole authority for the previous checkpoint.
            let prev_structured = input
                .messages
                .iter()
                .filter(|message| is_generated_summary(message))
                .filter_map(|message| message.content.as_text_ref())
                .filter_map(|text| text.strip_prefix("[对话历史摘要]\n"))
                .find_map(StructuredSummary::from_llm_response);

            // Decide: first compression or incremental?
            let (summary_text, structured) = if let Some(ref prev) = prev_structured {
                // Incremental path: summarize new messages, then merge field-by-field
                self.incremental_structured_summary(
                    &to_summarize,
                    prev,
                    focus.as_deref(),
                    input.cancel_token.clone(),
                )
                .await
            } else {
                // First compression: full structured summary (with natural language fallback)
                self.try_first_structured_summary(
                    &to_summarize,
                    focus.as_deref(),
                    input.cancel_token.clone(),
                )
                .await
            };

            check_cancelled(&input)?;

            let (final_text, _final_structured, summary_for_checkpoint) =
                match (summary_text, structured) {
                    (Some(_text), Some(ref s)) => {
                        // Structured summary succeeded — merge with previous and store
                        let merged = if let Some(prev) = prev_structured {
                            let mut m = prev;
                            m.merge_with(s);
                            m
                        } else {
                            s.clone()
                        };
                        let checkpoint_json = merged.to_json();
                        (merged.to_system_message(), Some(merged), checkpoint_json)
                    }
                    (Some(text), None) => {
                        // Natural language fallback
                        let content = format!("[对话历史摘要]\n{}", text);
                        (content, None, text)
                    }
                    (None, _) => {
                        // LLM failed entirely
                        warn!("Incremental summary LLM failed, falling back to sliding window");
                        return SlidingWindowCompressor::new(self.keep_recent)
                            .with_recent_token_budget(
                                self.recent_token_budget.unwrap_or(input.token_limit),
                            )
                            .compress(input)
                            .await;
                    }
                };

            let mut provisional = system_msgs;
            provisional.push(Message::system(final_text));
            provisional.extend(to_keep);
            let bounded_result = SlidingWindowCompressor::new(self.keep_recent)
                .with_recent_token_budget(input.token_limit)
                .compress(CompressionInput {
                    messages: provisional,
                    token_limit: input.token_limit,
                    current_query: input.current_query.clone(),
                    focus_instructions: input.focus_instructions.clone(),
                    cancel_token: input.cancel_token.clone(),
                    tokenizer: Some(tokenizer.clone()),
                })
                .await;
            let bounded = match bounded_result {
                Ok(value) => value,
                Err(_) => {
                    return SlidingWindowCompressor::new(self.keep_recent)
                        .with_recent_token_budget(
                            self.recent_token_budget.unwrap_or(input.token_limit),
                        )
                        .compress(input)
                        .await;
                }
            };
            let messages = bounded.messages;
            // Same candidate-prefix provenance rule as SummaryCompressor:
            // unpublished synthetic summaries are not historical evictions.
            let candidate_evicted = bounded.evicted.first().is_some_and(is_generated_summary);
            let mut evicted = to_summarize;
            evicted.extend(
                bounded
                    .evicted
                    .into_iter()
                    .skip(usize::from(candidate_evicted)),
            );

            let tokens_after: usize = messages
                .iter()
                .filter_map(|m| m.content.as_text())
                .map(|c| tokenizer.count_tokens(&c))
                .sum();

            let mut checkpoint = CompressionCheckpoint::new(self.name())
                .with_counts(messages.len(), evicted.len())
                .with_tokens(tokens_before, tokens_after)
                .with_duration_ms(start.elapsed().as_millis() as u64)
                .with_focus(focus);
            if !candidate_evicted {
                checkpoint = checkpoint.with_summary(summary_for_checkpoint);
            }

            Ok(CompressionOutput {
                messages,
                evicted,
                checkpoint: Some(checkpoint),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::llm::capabilities::ProviderCapabilities;
    use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse};
    use echo_core::tokenizer::HeuristicTokenizer;
    use futures::stream::BoxStream;

    struct StaticSummaryLlm;

    impl LlmClient for StaticSummaryLlm {
        fn chat(&self, _request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
            Box::pin(async {
                Ok(ChatResponse {
                    message: Message::assistant(
                        "Earlier work, decisions, constraints, and evidence are preserved here."
                            .to_string(),
                    ),
                    finish_reason: Some("stop".to_string()),
                    usage: None,
                    raw: echo_core::llm::types::ChatCompletionResponse::default(),
                })
            })
        }

        fn chat_stream(
            &self,
            _request: ChatRequest,
        ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>> {
            Box::pin(async {
                let stream: BoxStream<'static, Result<ChatChunk>> =
                    Box::pin(futures::stream::empty());
                Ok(stream)
            })
        }

        fn model_name(&self) -> &str {
            "static-summary"
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::anthropic()
        }
    }

    #[tokio::test]
    async fn token_summary_and_incremental_keep_recent_turns_across_repeated_compaction()
    -> Result<()> {
        let request = "current instruction 中文🚀";
        let initial = vec![
            Message::user("old ".repeat(100)),
            Message::assistant("old answer".to_string()),
            Message::user(request.to_string()),
            Message::assistant("recent work".to_string()),
        ];
        let strategies: Vec<Box<dyn ContextCompressor>> = vec![
            Box::new(
                SummaryCompressor::new(Arc::new(StaticSummaryLlm), 1).with_recent_token_budget(30),
            ),
            Box::new(
                IncrementalSummaryCompressor::new(Arc::new(StaticSummaryLlm), 1)
                    .with_recent_token_budget(30),
            ),
        ];
        for strategy in strategies {
            let mut messages = initial.clone();
            for _ in 0..3 {
                let output = strategy
                    .compress(CompressionInput {
                        messages,
                        token_limit: 100,
                        current_query: Some(request.to_string()),
                        focus_instructions: None,
                        cancel_token: None,
                        tokenizer: None,
                    })
                    .await?;
                assert!(message_tokens(&output.messages, &HeuristicTokenizer) <= 100);
                assert!(
                    output
                        .messages
                        .iter()
                        .any(|message| message.content.as_text_ref() == Some(request))
                );
                assert_eq!(
                    output
                        .messages
                        .iter()
                        .filter(|message| is_generated_summary(message))
                        .count(),
                    1
                );
                messages = output.messages;
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn summary_preserves_latest_request_before_long_tool_batch() -> Result<()> {
        use echo_core::llm::types::{FunctionCall, ToolCall};
        let request = "请保留当前用户的精确约束🚀";
        let mut call = Message::assistant(String::new());
        call.tool_calls = Some(vec![ToolCall {
            id: "lookup".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "lookup".to_string(),
                arguments: "{}".to_string(),
            },
        }]);
        let output = SummaryCompressor::new(Arc::new(StaticSummaryLlm), 2)
            .compress(CompressionInput {
                messages: vec![
                    Message::user("old request".to_string()),
                    Message::assistant("old answer".to_string()),
                    Message::user(request.to_string()),
                    call,
                    Message::tool_result(
                        "lookup".to_string(),
                        "lookup".to_string(),
                        "evidence".to_string(),
                    ),
                    Message::assistant("ongoing work".to_string()),
                ],
                token_limit: 200,
                current_query: Some(request.to_string()),
                focus_instructions: None,
                cancel_token: None,
                tokenizer: None,
            })
            .await?;
        assert!(
            output
                .messages
                .iter()
                .any(|message| message.content.as_text_ref() == Some(request))
        );
        Ok(())
    }

    #[tokio::test]
    async fn near_limit_request_wins_over_a_new_summary() -> Result<()> {
        let request = "r".repeat(360);
        let strategies: Vec<Box<dyn ContextCompressor>> = vec![
            Box::new(
                SummaryCompressor::new(Arc::new(StaticSummaryLlm), 20).with_recent_token_budget(20),
            ),
            Box::new(
                IncrementalSummaryCompressor::new(Arc::new(StaticSummaryLlm), 20)
                    .with_recent_token_budget(20),
            ),
        ];
        for strategy in strategies {
            let output = strategy
                .compress(CompressionInput {
                    messages: vec![
                        Message::system("[对话历史摘要]\nold summary ".repeat(20)),
                        Message::user("old ".repeat(100)),
                        Message::user(request.clone()),
                    ],
                    token_limit: 100,
                    current_query: Some(request.clone()),
                    focus_instructions: None,
                    cancel_token: None,
                    tokenizer: None,
                })
                .await?;
            assert!(
                output
                    .messages
                    .iter()
                    .any(|message| message.content.as_text_ref() == Some(request.as_str()))
            );
            assert!(message_tokens(&output.messages, &HeuristicTokenizer) <= 100);
            assert_eq!(output.evicted.len(), 2);
            assert_eq!(output.messages.len(), 1);
            assert!(output.checkpoint.as_ref().is_some_and(|checkpoint| {
                checkpoint.evicted_count == 2 && checkpoint.summary.is_none()
            }));
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejected_context_transform_keeps_incremental_observation_cache() -> Result<()> {
        use crate::compression::{ContextManager, MemoryPromoter, MemoryPromotionReceipt};
        struct SharedIncremental(Arc<IncrementalSummaryCompressor>);
        impl ContextCompressor for SharedIncremental {
            fn compress(
                &self,
                input: CompressionInput,
            ) -> BoxFuture<'_, Result<CompressionOutput>> {
                self.0.compress(input)
            }
            fn context_committed(&self, messages: &[Message]) {
                self.0.context_committed(messages);
            }
        }
        struct FailingPromotion;
        impl MemoryPromoter for FailingPromotion {
            fn promote(
                &self,
                _messages: &[Message],
            ) -> BoxFuture<'_, Result<MemoryPromotionReceipt>> {
                Box::pin(async {
                    Err(echo_core::error::AgentError::ContextLimitExceeded(
                        "promotion failed".to_string(),
                    )
                    .into())
                })
            }
        }
        let strategy = Arc::new(
            IncrementalSummaryCompressor::new(Arc::new(StaticSummaryLlm), 20)
                .with_recent_token_budget(30),
        );
        let accepted = StructuredSummary {
            goal: "accepted goal".to_string(),
            constraints: vec!["accepted constraint".to_string()],
            ..Default::default()
        };
        let mut context = ContextManager::builder(500)
            .compressor(SharedIncremental(strategy.clone()))
            .build();
        context.set_messages(vec![
            Message::system(accepted.to_system_message()),
            Message::user("old ".repeat(200)),
            Message::assistant("old result".repeat(40)),
            Message::user("current precise request".to_string()),
        ]);
        let cache = strategy.current_summary();
        let before = serde_json::to_value(context.messages())?;
        context.set_memory_promoter(Arc::new(FailingPromotion));
        assert!(context.force_compress(20).await.is_err());
        assert_eq!(strategy.current_summary(), cache);
        assert_eq!(before, serde_json::to_value(context.messages())?);
        context.remove_memory_promoter();
        context
            .force_compress_with(&SlidingWindowCompressor::new(2).with_recent_token_budget(30))
            .await?;
        assert!(strategy.current_summary().is_none());
        context.set_messages(vec![Message::system(accepted.to_system_message())]);
        assert!(strategy.current_summary().is_some());
        context.clear();
        assert!(strategy.current_summary().is_none());
        Ok(())
    }

    #[tokio::test]
    async fn summary_keeps_checkpoint_and_latest_request_within_token_limit() -> Result<()> {
        let compressor = SummaryCompressor::new(Arc::new(StaticSummaryLlm), 4);
        let latest_request = "latest request must survive";
        let messages = vec![
            Message::system("system".to_string()),
            Message::user("old request ".repeat(80)),
            Message::assistant("old answer ".repeat(80)),
            Message::user("large recent request ".repeat(80)),
            Message::assistant("large recent answer ".repeat(80)),
            Message::user(latest_request.to_string()),
        ];

        let output = compressor
            .compress(CompressionInput {
                messages,
                token_limit: 100,
                current_query: Some(latest_request.to_string()),
                focus_instructions: None,
                cancel_token: None,
                tokenizer: None,
            })
            .await?;
        let token_count = output.messages.iter().fold(0usize, |total, message| {
            total.saturating_add(message.content.estimated_tokens(&HeuristicTokenizer))
        });

        assert!(token_count <= 100, "bounded summary exceeded token limit");
        assert!(output.messages.iter().any(|message| {
            message
                .content
                .as_text_ref()
                .is_some_and(|text| text.contains(latest_request))
        }));
        assert!(output.messages.iter().any(is_generated_summary));
        assert!(
            output
                .checkpoint
                .as_ref()
                .and_then(|checkpoint| checkpoint.summary.as_deref())
                .is_some_and(|summary| !summary.is_empty())
        );
        Ok(())
    }

    #[test]
    fn test_incremental_summary_state_management() {
        // Test the Mutex-based state management without needing an LLM
        let previous_summary: Mutex<Option<String>> = Mutex::new(None);

        // Initially empty
        assert!(previous_summary.lock().unwrap().is_none());

        // Store a summary
        *previous_summary.lock().unwrap() = Some("first summary".to_string());
        assert_eq!(
            *previous_summary.lock().unwrap(),
            Some("first summary".to_string())
        );

        // Update the summary
        *previous_summary.lock().unwrap() = Some("updated summary".to_string());
        assert_eq!(
            *previous_summary.lock().unwrap(),
            Some("updated summary".to_string())
        );

        // Reset
        *previous_summary.lock().unwrap() = None;
        assert!(previous_summary.lock().unwrap().is_none());
    }
}
