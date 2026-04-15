//! LLM 客户端
//!
//! 统一的 LLM 抽象层，支持 OpenAI 兼容 API、自定义实现和 Mock 测试。
//!
//! # 核心类型
//!
//! - [`LlmClient`]: LLM 客户端 trait
//! - [`OpenAiClient`]: OpenAI 兼容客户端
//! - [`ChatRequest`][]: 聊天请求
//! - [`ChatResponse`][]: 聊天响应
//! - [`ChatChunk`][]: 流式响应块
//!
//! # 示例：简单对话
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let client = OpenAiClient::from_env("qwen3-max")?;
//!
//! let response = client.chat(ChatRequest {
//!     messages: vec![Message::user("你好".to_string())],
//!     ..Default::default()
//! }).await?;
//!
//! println!("{}", response.content().unwrap_or_default());
//! # Ok(())
//! # }
//! ```
//!
//! # 示例：流式对话
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//! use futures::StreamExt;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let client = OpenAiClient::from_env("qwen3-max")?;
//!
//! let mut stream = client.chat_stream(ChatRequest {
//!     messages: vec![Message::user("讲个笑话".to_string())],
//!     ..Default::default()
//! }).await?;
//!
//! while let Some(chunk) = stream.next().await {
//!     if let Some(content) = chunk?.delta.content {
//!         print!("{}", content);
//!     }
//! }
//! # Ok(())
//! # }
//! ```

pub mod types {
    //! OpenAI Chat Completions API 类型定义
    pub use echo_core::llm::types::*;
}

pub mod config {
    //! LLM 配置
    pub use echo_providers::config::*;
}

pub mod providers {
    //! LLM Provider 实现
    pub use echo_providers::anthropic::AnthropicClient;
    pub use echo_providers::ollama::OllamaClient;
}

// Core traits from echo-core
pub use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};

// Provider implementations from echo-providers
pub use echo_providers::openai::{
    DefaultLlmClient, OpenAiClient, assemble_req_header, chat, stream_chat,
};

// Config & Factory
pub use config::LlmConfig;
pub use echo_providers::ProviderFactory;

// Wire types for internal use
#[allow(unused_imports)]
pub(crate) use types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Message,
};
pub use types::{JsonSchemaSpec, Message as LlmMessage, ResponseFormat, ToolDefinition};
