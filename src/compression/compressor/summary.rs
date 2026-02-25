use crate::compression::{CompressionInput, CompressionOutput, ContextCompressor};
use crate::error::Result;
use crate::llm::LlmClient;
use crate::llm::types::Message;
use async_trait::async_trait;
use std::sync::Arc;

const COMPRESSION_PROMPT: &str =
    "你的任务是创建到目前为止对话的详细摘要，密切关注用户的明确请求和你之前的行动。
此摘要应彻底捕获需求细节和决策，这些对于在不丢失上下文的情况下继续开发工作至关重要。

在提供最终摘要之前，将你的分析包装在<analysis> </analysis>标签中，以组织你的思考并确保你涵盖了所有必要的要点。

你的摘要应包括以下部分:

1. **主要请求和意图**: 详细捕获用户的所有明确请求和意图
   - 用户明确说了什么？
   - 任务的核心目标是什么？
   - 有哪些隐含的需求？

2. **关键技术概念**: 列出讨论的所有重要技术概念、技术和框架
   - 使用了哪些技术栈？
   - 涉及哪些核心概念？
   - 有哪些重要的决策？

3. **任务和任务内容**: 枚举检查、修改或创建的特定任务
   - 特别注意最近的消息
   - 包含完整的任务详情

4. **错误和修复**: 列出你遇到的所有错误以及修复方法
   - 具体的错误信息
   - 解决方案和原因
   - 特别注意用户的反馈

5. **问题解决**: 记录已解决的问题和任何正在进行的故障排除工作
   - 解决了哪些难题？
   - 采用了什么方法？
   - 还有哪些未解决的问题？

6. **所有用户消息**: 列出所有非工具结果的用户消息
   - 这些对于理解用户的反馈和变化的意图至关重要
   - 按时间顺序列出
   - 注意用户态度和需求的变化

7. **待处理任务**: 概述你明确被要求处理的任何待处理任务
   - 哪些任务还没完成？
   - 优先级如何？
   - 有哪些依赖关系？

8. **当前工作**: 详细描述在此摘要请求之前正在进行的确切工作
   - 最后在做什么？
   - 进展到哪一步了？
   - 下一步计划做什么？

9. **可选的下一步**: 列出与你最近正在做的工作相关的下一步
   - 逻辑上的下一步是什么？
   - 有哪些可能的方向？

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
            .filter_map(|m| m.content.as_ref().map(|c| format!("[{}]: {}", m.role, c)))
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
/// let prompt = FnSummaryPrompt(|msgs| {
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

// ──────────────────────────────────────────────
// SummaryCompressor
// ──────────────────────────────────────────────

/// 摘要压缩：用 LLM 将较早的对话历史压缩成一条摘要 system 消息，保留最近 `keep_recent` 条不变。
///
/// 压缩后的消息结构：
/// ```
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

#[async_trait]
impl<P: SummaryPromptBuilder + 'static> ContextCompressor for SummaryCompressor<P> {
    async fn compress(&self, input: CompressionInput) -> Result<CompressionOutput> {
        let (system_msgs, conv_msgs): (Vec<_>, Vec<_>) =
            input.messages.into_iter().partition(|m| m.role == "system");

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
        let summary = self.llm.chat_simple(vec![Message::user(prompt)]).await?;

        let mut messages = system_msgs;
        messages.push(Message::system(format!("[对话历史摘要]\n{}", summary)));
        messages.extend(to_keep);

        Ok(CompressionOutput {
            messages,
            evicted: to_summarize.to_vec(),
        })
    }
}
