use crate::compression::compressor::SlidingWindowCompressor;
use crate::compression::{CompressionInput, CompressionOutput, ContextCompressor};
use echo_core::error::Result;
use echo_core::llm::LlmClient;
use echo_core::llm::types::Message;
use futures::future::BoxFuture;
use std::sync::Arc;
use tracing::warn;

/// Type alias for summary prompt builder closures
pub type SummaryPromptFn = Box<dyn Fn(&[Message]) -> String + Send + Sync>;

const COMPRESSION_PROMPT: &str =
    "你的任务是创建到目前为止对话的详细摘要，密切关注用户的明确请求和你之前的行动。
此摘要应彻底捕获需求细节和决策，这些对于在不丢失上下文的情况下继续开发工作至关重要。

你的摘要应包括以下部分:

1. **主要请求和意图**: 详细捕获用户的所有明确请求和意图
2. **关键技术概念**: 列出讨论的所有重要技术概念、技术和框架
3. **任务和任务内容**: 枚举检查、修改或创建的特定任务
4. **错误和修复**: 列出你遇到的所有错误以及修复方法
5. **问题解决**: 记录已解决的问题和任何正在进行的故障排除工作
6. **所有用户消息**: 列出所有非工具结果的用户消息
7. **待处理任务**: 概述你明确被要求处理的任何待处理任务
8. **当前工作**: 详细描述在此摘要请求之前正在进行的确切工作
9. **可选的下一步**: 列出与你最近正在做的工作相关的下一步

请确保摘要足够详细，使得另一个AI助手（或你自己在新会话中）能够无缝地继续这个对话和工作。
";

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
    let history = messages
        .iter()
        .filter_map(|m| m.content.as_text().map(|c| format!("[{}]: {}", m.role, c)))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "请将以下对话历史压缩为简洁的摘要。\
        要求：\n {}。\
        \n{}\n\n。",
        COMPRESSION_PROMPT, history
    )
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

impl ContextCompressor for SummaryCompressor {
    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        Box::pin(async move {
            let (system_msgs, conv_msgs): (Vec<_>, Vec<_>) = input
                .messages
                .iter()
                .cloned()
                .partition(|m| m.role == "system");

            if conv_msgs.len() <= self.keep_recent {
                let mut messages = system_msgs;
                messages.extend(conv_msgs);
                return Ok(CompressionOutput {
                    messages,
                    evicted: vec![],
                });
            }

            let split_at = conv_msgs.len() - self.keep_recent;
            let to_summarize = &conv_msgs[..split_at];
            let to_keep = conv_msgs[split_at..].to_vec();

            let prompt = (self.prompt_fn)(to_summarize);

            // 当 LLM 摘要生成失败（超时、API 错误等），回退到滑动窗口压缩
            let summary = match self.llm.chat_simple(vec![Message::user(prompt)]).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, "⚠️ LLM 摘要生成失败，回退到滑动窗口压缩");
                    return SlidingWindowCompressor::new(self.keep_recent)
                        .compress(input)
                        .await;
                }
            };

            let mut messages = system_msgs;
            messages.push(Message::system(format!("[对话历史摘要]\n{}", summary)));
            messages.extend(to_keep);

            Ok(CompressionOutput {
                messages,
                evicted: to_summarize.to_vec(),
            })
        })
    }
}
