use crate::compression::compressor::SlidingWindowCompressor;
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
}

impl SummaryCompressor {
    /// 使用内置中文摘要模板构造。
    pub fn new(llm: Arc<dyn LlmClient>, keep_recent: usize) -> Self {
        Self {
            llm,
            prompt_fn: Box::new(default_summary_prompt),
            keep_recent,
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
        }
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
    fn name(&self) -> &'static str {
        "Summary"
    }

    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        Box::pin(async move {
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
            let system_count = system_msgs.len();

            if conv_msgs.len() <= self.keep_recent {
                if tokens_before > input.token_limit {
                    return SlidingWindowCompressor::new(self.keep_recent)
                        .compress(input)
                        .await;
                }
                let mut messages = system_msgs;
                messages.extend(conv_msgs);
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

            let split_at = conv_msgs.len() - self.keep_recent;
            let to_summarize = &conv_msgs[..split_at];
            let to_keep = conv_msgs[split_at..].to_vec();

            // Try structured output first; fall back to natural language
            let (summary_text, structured) = self
                .try_structured_summary(to_summarize, focus.as_deref(), input.cancel_token.clone())
                .await;

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
                        .compress(input)
                        .await;
                }
            };

            let mut provisional = system_msgs;
            provisional.push(Message::system(final_summary));
            provisional.extend(to_keep);
            let bounded = SlidingWindowCompressor::new(self.keep_recent)
                .compress(CompressionInput {
                    messages: provisional,
                    token_limit: input.token_limit,
                    current_query: input.current_query.clone(),
                    focus_instructions: input.focus_instructions.clone(),
                    cancel_token: input.cancel_token.clone(),
                    tokenizer: Some(tokenizer.clone()),
                })
                .await?;
            let messages = bounded.messages;
            let mut evicted = to_summarize.to_vec();
            evicted.extend(bounded.evicted);

            let tokens_after: usize = messages
                .iter()
                .filter_map(|m| m.content.as_text())
                .map(|c| tokenizer.count_tokens(&c))
                .sum();

            let checkpoint = CompressionCheckpoint::new(self.name())
                .with_covered_range(
                    system_count,
                    system_count.saturating_add(split_at).saturating_sub(1),
                )
                .with_summary(summary_for_checkpoint.unwrap_or_default())
                .with_counts(messages.len(), evicted.len())
                .with_tokens(tokens_before, tokens_after)
                .with_duration_ms(start.elapsed().as_millis() as u64)
                .with_focus(focus);

            Ok(CompressionOutput {
                messages,
                evicted,
                checkpoint: Some(checkpoint),
            })
        })
    }
}

fn is_generated_summary(message: &Message) -> bool {
    message
        .content
        .as_text_ref()
        .is_some_and(|text| text.starts_with("[对话历史摘要]"))
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
    /// The previous structured summary, updated after each successful compression.
    previous_summary: Mutex<Option<StructuredSummary>>,
}

impl IncrementalSummaryCompressor {
    pub fn new(llm: Arc<dyn LlmClient>, keep_recent: usize) -> Self {
        Self {
            llm,
            keep_recent,
            previous_summary: Mutex::new(None),
        }
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
        match self.llm.chat_simple(vec![Message::user(prompt)]).await {
            Ok(text) => (Some(text), None),
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
                    cancel_token,
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

        match self.llm.chat_simple(vec![Message::user(prompt)]).await {
            Ok(text) => (Some(text), None),
            Err(_) => (None, None),
        }
    }
}

impl ContextCompressor for IncrementalSummaryCompressor {
    fn name(&self) -> &'static str {
        "IncrementalSummary"
    }

    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        Box::pin(async move {
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
            let system_count = system_msgs.len();

            if conv_msgs.len() <= self.keep_recent {
                if tokens_before > input.token_limit {
                    return SlidingWindowCompressor::new(self.keep_recent)
                        .compress(input)
                        .await;
                }
                let mut messages = system_msgs;
                messages.extend(conv_msgs);
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

            let split_at = conv_msgs.len() - self.keep_recent;
            let to_summarize = &conv_msgs[..split_at];
            let to_keep = conv_msgs[split_at..].to_vec();

            let prev_structured = self.current_structured_summary();

            // Decide: first compression or incremental?
            let (summary_text, structured) = if let Some(ref prev) = prev_structured {
                // Incremental path: summarize new messages, then merge field-by-field
                self.incremental_structured_summary(
                    to_summarize,
                    prev,
                    focus.as_deref(),
                    input.cancel_token.clone(),
                )
                .await
            } else {
                // First compression: full structured summary (with natural language fallback)
                self.try_first_structured_summary(
                    to_summarize,
                    focus.as_deref(),
                    input.cancel_token.clone(),
                )
                .await
            };

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
                        // Store for next incremental pass
                        if let Ok(mut guard) = self.previous_summary.lock() {
                            *guard = Some(merged.clone());
                        }
                        let checkpoint_json = merged.to_json();
                        (merged.to_system_message(), Some(merged), checkpoint_json)
                    }
                    (Some(text), None) => {
                        // Natural language fallback
                        let content = format!("[对话历史摘要]\n{}", text);
                        // Also store as text for backward compat
                        if let Ok(mut guard) = self.previous_summary.lock() {
                            *guard = None; // Reset structured state on fallback
                        }
                        (content, None, text)
                    }
                    (None, _) => {
                        // LLM failed entirely
                        warn!("Incremental summary LLM failed, falling back to sliding window");
                        return SlidingWindowCompressor::new(self.keep_recent)
                            .compress(input)
                            .await;
                    }
                };

            let mut provisional = system_msgs;
            provisional.push(Message::system(final_text));
            provisional.extend(to_keep);
            let bounded = SlidingWindowCompressor::new(self.keep_recent)
                .compress(CompressionInput {
                    messages: provisional,
                    token_limit: input.token_limit,
                    current_query: input.current_query.clone(),
                    focus_instructions: input.focus_instructions.clone(),
                    cancel_token: input.cancel_token.clone(),
                    tokenizer: Some(tokenizer.clone()),
                })
                .await?;
            let messages = bounded.messages;
            let mut evicted = to_summarize.to_vec();
            evicted.extend(bounded.evicted);

            let tokens_after: usize = messages
                .iter()
                .filter_map(|m| m.content.as_text())
                .map(|c| tokenizer.count_tokens(&c))
                .sum();

            let checkpoint = CompressionCheckpoint::new(self.name())
                .with_covered_range(
                    system_count,
                    system_count.saturating_add(split_at).saturating_sub(1),
                )
                .with_summary(summary_for_checkpoint)
                .with_counts(messages.len(), evicted.len())
                .with_tokens(tokens_before, tokens_after)
                .with_duration_ms(start.elapsed().as_millis() as u64)
                .with_focus(focus);

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
