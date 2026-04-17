use crate::compression::compressor::SlidingWindowCompressor;
use crate::compression::{CompressionInput, CompressionOutput, ContextCompressor};
use echo_core::error::Result;
use echo_core::llm::LlmClient;
use echo_core::llm::types::Message;
use futures::future::BoxFuture;
use std::sync::Arc;
use tracing::warn;

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

/// 摘要提示词构建接口，支持用户自定义摘要策略
pub trait SummaryPromptBuilder: Send + Sync {
    fn build(&self, messages: &[Message]) -> String;
}

/// 默认摘要提示词：指示 LLM 压缩对话历史，保留关键信息
pub struct DefaultSummaryPrompt;

impl SummaryPromptBuilder for DefaultSummaryPrompt {
    fn build(&self, messages: &[Message]) -> String {
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
}

/// 用闭包自定义提示词的便捷包装
///
/// # 示例
///
/// ```rust
/// use echo_core::llm::types::Message;
/// use echo_state::compression::compressor::summary::FnSummaryPrompt;
///
/// let prompt = FnSummaryPrompt(|msgs: &[Message]| {
///     format!("用一段话总结以下对话：\n{:?}", msgs)
/// });
/// ```
pub struct FnSummaryPrompt<F>(pub F)
where
    F: Fn(&[Message]) -> String + Send + Sync;

impl<F> SummaryPromptBuilder for FnSummaryPrompt<F>
where
    F: Fn(&[Message]) -> String + Send + Sync,
{
    fn build(&self, messages: &[Message]) -> String {
        (self.0)(messages)
    }
}

/// 摘要压缩：用 LLM 将较早的对话历史压缩成一条摘要 system 消息，保留最近 `keep_recent` 条不变。
///
/// 压缩后的消息结构：
/// ```text
/// [原有 system 消息]
/// [system] [对话历史摘要] <-- 新插入
/// [最近 keep_recent 条对话消息]
/// ```
///
/// 适用场景：
/// - 长线任务规划（将已完成步骤压缩为状态摘要）
/// - 需要记住角色设定和重大事件，但不需要保留全部细节
pub struct SummaryCompressor<P: SummaryPromptBuilder> {
    llm: Arc<dyn LlmClient>,
    prompt_builder: P,
    /// 最近多少条对话消息保持原样（不参与摘要）
    keep_recent: usize,
}

impl<P: SummaryPromptBuilder> SummaryCompressor<P> {
    pub fn new(llm: Arc<dyn LlmClient>, prompt_builder: P, keep_recent: usize) -> Self {
        Self {
            llm,
            prompt_builder,
            keep_recent,
        }
    }
}

impl<P: SummaryPromptBuilder + 'static> ContextCompressor for SummaryCompressor<P> {
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

            let prompt = self.prompt_builder.build(to_summarize);

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
