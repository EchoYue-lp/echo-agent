//! 上下文压缩
//!
//! 维护对话历史并在 token 超限时自动压缩，由 [`ContextManager`] 统一管理。
//!
//! 内置压缩策略（均实现 [`ContextCompressor`] trait）：
//! - [`compressor::SlidingWindowCompressor`]：滑动窗口，丢弃最早的 N 条消息
//! - [`compressor::SummaryCompressor`]：LLM 摘要，将旧消息压缩为 system 摘要消息
//! - [`compressor::HybridCompressor`]：多策略串联管道

pub mod compressor;

use crate::compression::compressor::SlidingWindowCompressor;
use echo_core::error::Result;
use echo_core::llm::types::{Message, MessageContent};
use echo_core::tokenizer::{HeuristicTokenizer, Tokenizer};
use futures::future::BoxFuture;
use std::sync::Arc;

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

/// Metadata needed to restore protected messages near their original positions.
struct ProtectedMessage {
    message: Message,
    /// Number of compressible messages that originally appeared after this message.
    compressible_after: usize,
    /// Number of protected messages that originally appeared after this message.
    protected_after: usize,
}

/// 所有压缩策略的统一接口（async，支持 `dyn` trait object）
pub trait ContextCompressor: Send + Sync {
    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>>;
}

/// 允许将 `Box<dyn ContextCompressor>` 直接传给任何接受 `impl ContextCompressor` 的函数，
/// 无需引入额外的包装枚举。
impl ContextCompressor for Box<dyn ContextCompressor> {
    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        (**self).compress(input)
    }
}

/// `force_compress()` 返回的压缩统计信息
pub struct ForceCompressStats {
    /// 压缩前消息总数
    pub before_count: usize,
    /// 压缩后消息总数
    pub after_count: usize,
    /// 被裁剪掉的消息数
    pub evicted: usize,
    /// 压缩前估算 token 数
    pub before_tokens: usize,
    /// 压缩后估算 token 数
    pub after_tokens: usize,
}

/// 上下文管理器：维护完整对话历史，并在 token 超限时自动触发压缩。
///
/// # 典型用法
///
/// ```rust,no_run
/// use echo_core::error::Result;
/// use echo_core::llm::types::Message;
/// use echo_state::compression::compressor::SlidingWindowCompressor;
/// use echo_state::compression::{ContextCompressor, ContextManager};
///
/// # async fn example() -> Result<()> {
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
/// use echo_core::error::Result;
/// use echo_core::llm::LlmClient;
/// use echo_state::compression::compressor::{
///     HybridCompressor, SlidingWindowCompressor, SummaryCompressor, DefaultSummaryPrompt,
/// };
/// use echo_state::compression::{ContextCompressor, ContextManager};
/// use std::sync::Arc;
///
/// # async fn example(llm: Arc<dyn LlmClient>) -> Result<()> {
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
    tokenizer: Arc<dyn Tokenizer>,
    /// Content markers that identify protected messages (survive compaction).
    /// Any message whose content contains one of these markers is excluded from compression.
    /// Used by the skill system to protect activated skill instructions.
    protected_markers: Vec<String>,
    /// 硬性消息数量上限。超过时触发 sliding window 降级，防止 OOM。
    /// 默认 200 条。
    max_messages: usize,
}

impl ContextManager {
    pub fn builder(token_limit: usize) -> ContextManagerBuilder {
        ContextManagerBuilder {
            token_limit,
            compressor: None,
            initial_messages: Vec::new(),
            tokenizer: None,
            max_messages: None,
        }
    }

    /// 追加一条消息到上下文缓冲区。
    ///
    /// 当消息数超过 `max_messages` 硬性上限时，自动应用 sliding window 降级：
    /// 保留 system 消息和最近的消息，丢弃中间最早的对话消息。
    /// 这是最后的防线，即使压缩器未配置或压缩失败也不会 OOM。
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);

        // 硬性上限降级：超过 max_messages 时应用 sliding window
        if self.messages.len() > self.max_messages {
            self.apply_hard_cap();
        }
    }

    /// 应用硬性消息上限：保留 system 消息、受保护消息和最近的消息，丢弃中间最早的。
    fn apply_hard_cap(&mut self) {
        let target = self.max_messages;
        if self.messages.len() <= target {
            return;
        }

        // 识别受保护的消息（不应被删除）
        let mut protected_indices: Vec<usize> = Vec::new();
        for (i, msg) in self.messages.iter().enumerate() {
            if self.is_protected(msg) {
                protected_indices.push(i);
            }
        }

        // 找到第一条非 system 消息的位置
        let first_non_system = self
            .messages
            .iter()
            .position(|m| m.role != "system")
            .unwrap_or(0);

        // 计算需要删除多少条非受保护的消息
        let excess = self.messages.len() - target;
        let mut to_remove = Vec::new();
        let mut removed = 0;
        for i in first_non_system..self.messages.len() {
            if removed >= excess {
                break;
            }
            // 跳过受保护的消息
            if protected_indices.contains(&i) {
                continue;
            }
            to_remove.push(i);
            removed += 1;
        }

        if to_remove.is_empty() {
            return;
        }

        tracing::warn!(
            total = self.messages.len(),
            cap = target,
            evicted = to_remove.len(),
            "📦 消息数超过硬性上限，应用 sliding window 降级（保留受保护消息）"
        );

        // 从后往前删除，避免索引偏移
        for &i in to_remove.iter().rev() {
            self.messages.remove(i);
        }
    }

    /// 批量追加消息
    pub fn push_many(&mut self, messages: impl IntoIterator<Item = Message>) {
        self.messages.extend(messages);
    }

    /// 返回当前缓冲区中的所有消息（不做压缩）
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// 替换内部消息缓冲区（用于从持久化存储恢复对话）
    ///
    /// 消息应包含 system prompt 作为第一条（如需要）。
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// 估算当前上下文的 token 数
    ///
    /// 使用已配置的 [`Tokenizer`] 实现（默认 [`HeuristicTokenizer`]，区分 ASCII/CJK）。
    pub fn token_estimate(&self) -> usize {
        Self::estimate_tokens(&self.messages, &*self.tokenizer)
    }

    /// 获取当前 Tokenizer
    pub fn tokenizer(&self) -> &dyn Tokenizer {
        &*self.tokenizer
    }

    /// 动态替换 Tokenizer
    pub fn set_tokenizer(&mut self, tokenizer: Arc<dyn Tokenizer>) {
        self.tokenizer = tokenizer;
    }

    /// 清空上下文缓冲区（保留已设置的压缩器和保护标记）
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Register a content marker that protects messages from compression.
    ///
    /// Any message whose content contains this marker string will be excluded
    /// from compression passes. This is used by the skill system to protect
    /// activated skill instructions from being evicted during context compaction.
    ///
    /// # Example
    /// ```rust,no_run
    /// # use echo_state::compression::ContextManager;
    /// let mut ctx = ContextManager::builder(4096).build();
    /// ctx.add_protected_marker("<skill_content".to_string());
    /// ```
    pub fn add_protected_marker(&mut self, marker: String) {
        if !self.protected_markers.contains(&marker) {
            self.protected_markers.push(marker);
        }
    }

    /// Check if a message is protected from compression.
    fn is_protected(&self, message: &Message) -> bool {
        if self.protected_markers.is_empty() {
            return false;
        }
        if let Some(content) = message.content.as_text() {
            self.protected_markers.iter().any(|m| content.contains(m))
        } else {
            false
        }
    }

    /// Split messages into (compressible, protected_metadata).
    ///
    /// Protected messages are removed from the compressible set and will be
    /// re-inserted at their original relative positions after compression.
    fn split_protected(&self, messages: Vec<Message>) -> (Vec<Message>, Vec<ProtectedMessage>) {
        let mut compressible = Vec::new();
        let mut protected: Vec<(usize, Message)> = Vec::new();
        let mut compressible_seen = 0usize;

        for msg in messages {
            if self.is_protected(&msg) {
                protected.push((compressible_seen, msg));
            } else {
                compressible.push(msg);
                compressible_seen += 1;
            }
        }

        let total_compressible = compressible.len();
        let total_protected = protected.len();
        let protected = protected
            .into_iter()
            .enumerate()
            .map(|(idx, (compressible_before, message))| ProtectedMessage {
                message,
                compressible_after: total_compressible.saturating_sub(compressible_before),
                protected_after: total_protected.saturating_sub(idx + 1),
            })
            .collect();

        (compressible, protected)
    }

    /// Merge protected messages back into the compressed output.
    ///
    /// Protected messages are re-inserted near their original relative positions.
    /// We restore from the tail so each message can reserve the amount of trailing
    /// conversation that originally followed it.
    fn merge_protected(compressed: Vec<Message>, protected: Vec<ProtectedMessage>) -> Vec<Message> {
        if protected.is_empty() {
            return compressed;
        }

        let mut result = compressed;
        for protected_msg in protected.into_iter().rev() {
            let trailing_slots = protected_msg.compressible_after + protected_msg.protected_after;
            let insert_at = result.len().saturating_sub(trailing_slots);
            result.insert(insert_at, protected_msg.message);
        }
        result
    }

    /// 动态替换压缩器，不影响已有的消息缓冲区
    pub fn set_compressor(&mut self, compressor: impl ContextCompressor + 'static) {
        self.compressor = Some(Box::new(compressor));
    }

    /// 移除压缩器，恢复为无限制模式
    pub fn remove_compressor(&mut self) {
        self.compressor = None;
    }

    /// 是否已配置压缩器
    pub fn has_compressor(&self) -> bool {
        self.compressor.is_some()
    }

    /// 强制压缩上下文，无视当前 token 用量是否超限。
    ///
    /// - 若已配置压缩器，使用当前压缩器执行；
    /// - 若未配置，则临时使用 `SlidingWindowCompressor::new(fallback_window)` 执行。
    ///
    /// Protected messages are excluded from compression and preserved.
    pub async fn force_compress(&mut self, fallback_window: usize) -> Result<ForceCompressStats> {
        let before_count = self.messages.len();
        let before_tokens = self.token_estimate();

        let (compressible, protected) = self.split_protected(self.messages.clone());

        let output = if let Some(compressor) = &self.compressor {
            let input = CompressionInput {
                messages: compressible,
                token_limit: self.token_limit,
                current_query: None,
            };
            compressor.compress(input).await?
        } else {
            SlidingWindowCompressor::new(fallback_window)
                .compress(CompressionInput {
                    messages: compressible,
                    token_limit: self.token_limit,
                    current_query: None,
                })
                .await?
        };

        let evicted = output.evicted.len();
        self.messages = Self::merge_protected(output.messages, protected);
        Ok(ForceCompressStats {
            before_count,
            after_count: self.messages.len(),
            evicted,
            before_tokens,
            after_tokens: self.token_estimate(),
        })
    }

    /// 强制使用**指定压缩器**压缩上下文，不影响已安装的压缩器配置。
    ///
    /// 适合 `/compress sliding 10` 这类临时覆盖策略的场景。
    pub async fn force_compress_with(
        &mut self,
        compressor: &dyn ContextCompressor,
    ) -> Result<ForceCompressStats> {
        let before_count = self.messages.len();
        let before_tokens = self.token_estimate();

        let (compressible, protected) = self.split_protected(self.messages.clone());

        let output = compressor
            .compress(CompressionInput {
                messages: compressible,
                token_limit: self.token_limit,
                current_query: None,
            })
            .await?;

        let evicted = output.evicted.len();
        self.messages = Self::merge_protected(output.messages, protected);
        Ok(ForceCompressStats {
            before_count,
            after_count: self.messages.len(),
            evicted,
            before_tokens,
            after_tokens: self.token_estimate(),
        })
    }

    /// 更新 system 消息内容
    ///
    /// 通常在 `add_skill()` 注入额外系统提示时调用：
    /// 找到第一条 role == "system" 的消息并替换其内容；
    /// 若不存在 system 消息，则在队列头部插入一条。
    pub fn update_system(&mut self, new_system_prompt: String) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.role == "system") {
            msg.content = MessageContent::Text(new_system_prompt);
        } else {
            self.messages.insert(0, Message::system(new_system_prompt));
        }
    }

    /// 准备发送给 LLM 的消息列表。
    ///
    /// 当估算 token 超过 `token_limit` 且已配置压缩器时，自动触发压缩并更新内部缓冲区。
    /// 压缩后的消息会替换原有缓冲区。
    ///
    /// Protected messages (containing registered markers, e.g. `<skill_content>`) are
    /// excluded from compression and re-inserted after system messages.
    ///
    /// `current_query` 为保留字段，传 `None` 即可。
    pub async fn prepare(&mut self, current_query: Option<&str>) -> Result<Vec<Message>> {
        if let Some(compressor) = &self.compressor
            && Self::estimate_tokens(&self.messages, &*self.tokenizer) > self.token_limit
        {
            let (compressible, protected) = self.split_protected(self.messages.clone());

            let output = compressor
                .compress(CompressionInput {
                    messages: compressible,
                    token_limit: self.token_limit,
                    current_query: current_query.map(String::from),
                })
                .await?;

            self.messages = Self::merge_protected(output.messages, protected);
        }
        Ok(self.messages.clone())
    }

    fn estimate_tokens(messages: &[Message], tokenizer: &dyn Tokenizer) -> usize {
        messages
            .iter()
            .filter_map(|m| m.content.as_text())
            .map(|c| tokenizer.count_tokens(&c))
            .sum()
    }
}

/// `ContextManager` 的构建器
pub struct ContextManagerBuilder {
    token_limit: usize,
    compressor: Option<Box<dyn ContextCompressor>>,
    initial_messages: Vec<Message>,
    tokenizer: Option<Arc<dyn Tokenizer>>,
    max_messages: Option<usize>,
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

    /// 设置自定义 Tokenizer（默认 [`HeuristicTokenizer`]）
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_state::compression::ContextManager;
    /// use echo_core::tokenizer::SimpleTokenizer;
    /// use std::sync::Arc;
    ///
    /// let ctx = ContextManager::builder(4096)
    ///     .tokenizer(Arc::new(SimpleTokenizer))
    ///     .build();
    /// ```
    pub fn tokenizer(mut self, tokenizer: Arc<dyn Tokenizer>) -> Self {
        self.tokenizer = Some(tokenizer);
        self
    }

    /// 设置消息数量硬性上限（默认 200）。
    ///
    /// 超过此上限时自动应用 sliding window 降级，保留 system 消息和最近的消息。
    pub fn max_messages(mut self, max: usize) -> Self {
        self.max_messages = Some(max);
        self
    }

    pub fn build(self) -> ContextManager {
        ContextManager {
            messages: self.initial_messages,
            compressor: self.compressor,
            token_limit: self.token_limit,
            tokenizer: self
                .tokenizer
                .unwrap_or_else(|| Arc::new(HeuristicTokenizer)),
            protected_markers: Vec::new(),
            max_messages: self.max_messages.unwrap_or(200),
        }
    }
}

#[cfg(test)]
mod tests {
    const MODEL: &str = "qwen3-max";
    use super::*;
    use crate::compression::compressor::{
        DefaultSummaryPrompt, FnSummaryPrompt, HybridCompressor, SlidingWindowCompressor,
        SummaryCompressor,
    };
    use echo_core::error::Result;
    use echo_integration::providers::openai::DefaultLlmClient;
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
            println!("  [{}] {}", m.role, m.content.as_text_ref().unwrap_or(""));
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_protected_messages_keep_relative_position_after_compression() -> Result<()> {
        let mut ctx = ContextManager::builder(10)
            .compressor(SlidingWindowCompressor::new(2))
            .build();
        ctx.add_protected_marker("<skill>".to_string());

        ctx.push(Message::system("system".to_string()));
        ctx.push(Message::user("old user".to_string()));
        ctx.push(Message::assistant("old assistant".to_string()));
        ctx.push(Message::user("<skill> protected".to_string()));
        ctx.push(Message::assistant("recent assistant".to_string()));
        ctx.push(Message::user("latest user".to_string()));

        let messages = ctx.force_compress(2).await?;
        assert!(messages.after_count >= 3);

        let rendered: Vec<(String, String)> = ctx
            .messages()
            .iter()
            .map(|m| {
                (
                    m.role.clone(),
                    m.content.as_text_ref().unwrap_or("").to_string(),
                )
            })
            .collect();

        assert_eq!(
            rendered,
            vec![
                ("system".to_string(), "system".to_string()),
                ("user".to_string(), "<skill> protected".to_string()),
                ("assistant".to_string(), "recent assistant".to_string()),
                ("user".to_string(), "latest user".to_string()),
            ]
        );

        Ok(())
    }

    #[tokio::test]
    #[ignore] // 需要 LLM API
    async fn test_summary_compressor_default_prompt() -> Result<()> {
        println!("\n=== 示例 2：摘要压缩（使用系统默认摘要提示词） ===");
        let http = Arc::new(Client::new());
        let llm = Arc::new(DefaultLlmClient::new(http, MODEL));

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
                    .as_text_ref()
                    .unwrap_or("")
                    .chars()
                    .take(80000)
                    .collect::<String>()
            );
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore] // 需要 LLM API
    async fn test_summary_compressor_fn_prompt() -> Result<()> {
        println!("\n=== 示例 3：摘要压缩（使用自定义摘要提示词） ===");
        let http = Arc::new(Client::new());
        let llm = Arc::new(DefaultLlmClient::new(http, MODEL));

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
                    .as_text_ref()
                    .unwrap_or("")
                    .chars()
                    .take(80000)
                    .collect::<String>()
            );
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore] // 需要 LLM API
    async fn test_hybrid_compressor() -> Result<()> {
        println!("\n=== 示例 4：混合管道（滑动窗口 → 摘要） ===");
        let http = Arc::new(Client::new());
        let llm = Arc::new(DefaultLlmClient::new(http, MODEL));

        let hybrid = HybridCompressor::builder()
            .stage(SlidingWindowCompressor::new(6))
            .stage(SummaryCompressor::new(llm.clone(), DefaultSummaryPrompt, 2))
            .build();

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
                    .as_text_ref()
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect::<String>()
            );
        }
        Ok(())
    }
}
