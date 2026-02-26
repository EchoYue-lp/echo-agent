pub mod compressor;

use crate::error::Result;
use crate::llm::types::Message;
use async_trait::async_trait;

// ──────────────────────────────────────────────
// 核心 trait 与数据结构
// ──────────────────────────────────────────────

/// 压缩管道的输入
pub struct CompressionInput {
    /// 待压缩的消息列表
    pub messages: Vec<Message>,
    /// Token 上限，超过时触发压缩
    pub token_limit: usize,
    /// 当前用户问题（保留字段，供扩展使用）
    pub current_query: Option<String>,
}

/// 压缩管道的输出
pub struct CompressionOutput {
    /// 最终保留、将发送给 LLM 的消息列表
    pub messages: Vec<Message>,
    /// 本次被裁剪掉的消息
    pub evicted: Vec<Message>,
}

/// 所有压缩策略的统一接口（async，支持 `dyn` trait object）
#[async_trait]
pub trait ContextCompressor: Send + Sync {
    async fn compress(&self, input: CompressionInput) -> Result<CompressionOutput>;
}

/// 上下文管理器：维护完整对话历史，并在 token 超限时自动触发压缩。
///
/// # 典型用法
///
/// ```rust,no_run
/// use echo_agent::compression::{ContextManager, ContextCompressor};
/// use echo_agent::compression::compressor::SlidingWindowCompressor;
/// use echo_agent::llm::types::Message;
///
/// # async fn example() -> echo_agent::error::Result<()> {
/// let mut ctx = ContextManager::builder(4096)
///     .compressor(SlidingWindowCompressor::new(20))
///     .build();
///
/// ctx.push(Message::system("你是一个助手".to_string()));
/// ctx.push(Message::user("你好".to_string()));
///
/// // 在每次调用 LLM 前调用 prepare()，自动压缩超限消息
/// let messages = ctx.prepare(None).await?;
/// # Ok(())
/// # }
/// ```
///
/// # 混合管道示例
///
/// ```rust,no_run
/// use echo_agent::compression::ContextManager;
/// use echo_agent::compression::compressor::{
///     HybridCompressor, SlidingWindowCompressor, SummaryCompressor, DefaultSummaryPrompt,
/// };
/// use std::sync::Arc;
///
/// # async fn example() -> echo_agent::error::Result<()> {
/// let compressor = HybridCompressor::builder()
///     .stage(SlidingWindowCompressor::new(30))
///     .stage(SummaryCompressor::new(llm, DefaultSummaryPrompt, 8))
///     .build();
///
/// let mut ctx = ContextManager::builder(8192)
///     .compressor(compressor)
///     .build();
/// # Ok(())
/// # }
/// ```
pub struct ContextManager {
    messages: Vec<Message>,
    compressor: Option<Box<dyn ContextCompressor>>,
    token_limit: usize,
}

impl ContextManager {
    pub fn builder(token_limit: usize) -> ContextManagerBuilder {
        ContextManagerBuilder {
            token_limit,
            compressor: None,
            initial_messages: Vec::new(),
        }
    }

    /// 追加一条消息到上下文缓冲区
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// 批量追加消息
    pub fn push_many(&mut self, messages: impl IntoIterator<Item = Message>) {
        self.messages.extend(messages);
    }

    /// 返回当前缓冲区中的所有消息（不做压缩）
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// 估算当前上下文的 token 数（粗略估算：字符数 / 4）
    pub fn token_estimate(&self) -> usize {
        Self::estimate_tokens(&self.messages)
    }

    /// 清空上下文缓冲区（保留已设置的压缩器）
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// 动态替换压缩器，不影响已有的消息缓冲区
    pub fn set_compressor(&mut self, compressor: impl ContextCompressor + 'static) {
        self.compressor = Some(Box::new(compressor));
    }

    /// 移除压缩器，恢复为无限制模式
    pub fn remove_compressor(&mut self) {
        self.compressor = None;
    }

    /// 更新 system 消息内容
    ///
    /// 通常在 `add_skill()` 注入额外系统提示时调用：
    /// 找到第一条 role == "system" 的消息并替换其内容；
    /// 若不存在 system 消息，则在队列头部插入一条。
    pub fn update_system(&mut self, new_system_prompt: String) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.role == "system") {
            msg.content = Some(new_system_prompt);
        } else {
            self.messages.insert(0, Message::system(new_system_prompt));
        }
    }

    /// 准备发送给 LLM 的消息列表。
    ///
    /// 当估算 token 超过 `token_limit` 且已配置压缩器时，自动触发压缩并更新内部缓冲区。
    /// 压缩后的消息会替换原有缓冲区。
    ///
    /// `current_query` 为保留字段，传 `None` 即可。
    pub async fn prepare(&mut self, current_query: Option<&str>) -> Result<Vec<Message>> {
        if let Some(compressor) = &self.compressor {
            if Self::estimate_tokens(&self.messages) > self.token_limit {
                let output = compressor
                    .compress(CompressionInput {
                        messages: self.messages.clone(),
                        token_limit: self.token_limit,
                        current_query: current_query.map(String::from),
                    })
                    .await?;
                self.messages = output.messages;
            }
        }
        Ok(self.messages.clone())
    }

    fn estimate_tokens(messages: &[Message]) -> usize {
        messages
            .iter()
            .filter_map(|m| m.content.as_ref())
            .map(|c| c.len() / 4 + 1)
            .sum()
    }
}

/// `ContextManager` 的构建器
pub struct ContextManagerBuilder {
    token_limit: usize,
    compressor: Option<Box<dyn ContextCompressor>>,
    initial_messages: Vec<Message>,
}

impl ContextManagerBuilder {
    /// 设置压缩策略（可选）。支持任意实现了 `ContextCompressor` 的类型，
    /// 包括 `SlidingWindowCompressor`、`SummaryCompressor` 和 `HybridCompressor`。
    pub fn compressor(mut self, c: impl ContextCompressor + 'static) -> Self {
        self.compressor = Some(Box::new(c));
        self
    }

    /// 预置一条 system 消息作为初始上下文（通常用于 Agent 的系统提示词）
    pub fn with_system(mut self, system_prompt: String) -> Self {
        self.initial_messages.push(Message::system(system_prompt));
        self
    }

    pub fn build(self) -> ContextManager {
        ContextManager {
            messages: self.initial_messages,
            compressor: self.compressor,
            token_limit: self.token_limit,
        }
    }
}

#[cfg(test)]
mod tests {
    const MODEL: &str = "qwen3-max";
    use super::*;
    use crate::error::Result;
    use crate::llm::DefaultLlmClient;
    use crate::prelude::{
        DefaultSummaryPrompt, FnSummaryPrompt, HybridCompressor, SlidingWindowCompressor,
        SummaryCompressor,
    };
    use reqwest::Client;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_sliding_window_compressor() -> Result<()> {
        println!("=== 示例 1：滑动窗口压缩 ===");

        let mut ctx = ContextManager::builder(200)
            .compressor(SlidingWindowCompressor::new(4))
            .build();

        ctx.push(Message::system("你是一个助手。".to_string()));
        for i in 1..=6 {
            ctx.push(Message::user(format!("用户消息 {}", i)));
            ctx.push(Message::assistant(format!("助手回复 {}", i)));
        }

        println!("压缩前消息数：{}", ctx.messages().len());
        let messages = ctx.prepare(None).await?;
        println!("压缩后消息数：{}", messages.len());
        for m in &messages {
            println!("  [{}] {}", m.role, m.content.as_deref().unwrap_or(""));
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_summary_compressor_default_prompt() -> Result<()> {
        // ──────────────────────────────────────────────
        // 示例 2：摘要压缩（使用系统默认摘要提示词）
        // ──────────────────────────────────────────────
        println!("\n=== 示例 2：摘要压缩（使用系统默认摘要提示词） ===");
        let http = Arc::new(Client::new());
        let llm = Arc::new(DefaultLlmClient::new(http, MODEL));

        // token_limit 设得很小，确保触发压缩
        let mut ctx2 = ContextManager::builder(50)
            .compressor(SummaryCompressor::new(llm.clone(), DefaultSummaryPrompt, 2))
            .build();

        ctx2.push(Message::system("你是任务规划助手。".to_string()));
        ctx2.push(Message::user("我想学习 Rust 语言".to_string()));
        ctx2.push(Message::assistant(
            "好的，Rust 是一门系统编程语言，以内存安全著称。建议从官方 The Book 开始。".to_string(),
        ));
        ctx2.push(Message::user("所有权机制怎么理解？".to_string()));
        ctx2.push(Message::assistant(
            "所有权是 Rust 核心概念：每个值都有唯一的所有者，所有者离开作用域时值被释放。"
                .to_string(),
        ));
        ctx2.push(Message::user("借用和引用又是什么？".to_string()));
        ctx2.push(Message::assistant(
            "借用允许你使用值但不取得所有权：不可变借用（&T）可以有多个，可变借用（&mut T）只能有一个。"
                .to_string(),
        ));

        println!("压缩前消息数：{}", ctx2.messages().len());
        println!("预估 token：{}", ctx2.token_estimate());

        let messages2 = ctx2.prepare(None).await?;

        println!("压缩后消息数：{}", messages2.len());
        for m in &messages2 {
            println!(
                "  [{}] {}",
                m.role,
                m.content
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(80000)
                    .collect::<String>()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_summary_compressor_fn_prompt() -> Result<()> {
        // ──────────────────────────────────────────────
        // 示例 3：摘要压缩（使用自定义摘要提示词）
        // ──────────────────────────────────────────────
        println!("\n=== 示例 3：摘要压缩（使用自定义摘要提示词） ===");
        let http = Arc::new(Client::new());
        let llm = Arc::new(DefaultLlmClient::new(http, MODEL));

        // token_limit 设得很小，确保触发压缩
        let mut ctx2 = ContextManager::builder(50)
            .compressor(SummaryCompressor::new(
                llm.clone(),
                FnSummaryPrompt(|_| "请用中文总结本对话".to_string()),
                2,
            ))
            .build();

        ctx2.push(Message::system("你是任务规划助手。".to_string()));
        ctx2.push(Message::user("我想学习 Rust 语言".to_string()));
        ctx2.push(Message::assistant(
            "好的，Rust 是一门系统编程语言，以内存安全著称。建议从官方 The Book 开始。".to_string(),
        ));
        ctx2.push(Message::user("所有权机制怎么理解？".to_string()));
        ctx2.push(Message::assistant(
            "所有权是 Rust 核心概念：每个值都有唯一的所有者，所有者离开作用域时值被释放。"
                .to_string(),
        ));
        ctx2.push(Message::user("借用和引用又是什么？".to_string()));
        ctx2.push(Message::assistant(
            "借用允许你使用值但不取得所有权：不可变借用（&T）可以有多个，可变借用（&mut T）只能有一个。"
                .to_string(),
        ));

        println!("压缩前消息数：{}", ctx2.messages().len());
        println!("预估 token：{}", ctx2.token_estimate());

        let messages2 = ctx2.prepare(None).await?;

        println!("压缩后消息数：{}", messages2.len());
        for m in &messages2 {
            println!(
                "  [{}] {}",
                m.role,
                m.content
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(80000)
                    .collect::<String>()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_hybrid_compressor() -> Result<()> {
        // ──────────────────────────────────────────────
        // 示例 4：混合管道（SlidingWindow → Summary）
        // ──────────────────────────────────────────────
        println!("\n=== 示例 4：混合管道（滑动窗口 → 摘要） ===");
        let http = Arc::new(Client::new());
        let llm = Arc::new(DefaultLlmClient::new(http, MODEL));

        let hybrid = HybridCompressor::builder()
            .stage(SlidingWindowCompressor::new(6))
            .stage(SummaryCompressor::new(llm.clone(), DefaultSummaryPrompt, 2))
            .build();

        // token_limit 设得很小，确保两个阶段都被触发
        let mut ctx3 = ContextManager::builder(80).compressor(hybrid).build();

        ctx3.push(Message::system("你是一个项目管理助手。".to_string()));
        for i in 1..=8 {
            ctx3.push(Message::user(format!("任务 {} 的进展如何？", i)));
            ctx3.push(Message::assistant(format!(
                "任务 {} 已完成，耗时约 {} 小时，质量良好。",
                i,
                i * 2
            )));
        }

        println!("压缩前消息数：{}", ctx3.messages().len());
        println!("预估 token：{}", ctx3.token_estimate());

        let messages3 = ctx3.prepare(None).await?;

        println!("压缩后消息数：{}", messages3.len());
        for m in &messages3 {
            println!(
                "  [{}] {}",
                m.role,
                m.content
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect::<String>()
            );
        }
        Ok(())
    }
}
