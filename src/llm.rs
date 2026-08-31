//! LLM client façade — provider abstraction and chat APIs.
//!
//! Unified interface over multiple LLM providers. All clients implement the
//! [`LlmClient`] trait, providing both `chat()` and `chat_stream()` methods.
//!
//! # Supported Protocols
//!
//! | Client | Protocol | Feature |
//! |--------|---------|---------|
//! | [`OpenAiClient`] | OpenAI & compatible APIs | default |
//! | [`AnthropicClient`] | Native Claude API | `a2a` |
//! | [`ResponsesClient`] | OpenAI Responses | default |
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let config = LlmConfig::for_provider(
//!     "local",
//!     "http://127.0.0.1:11434/v1",
//!     "",
//!     "qwen3",
//!     LlmApiProtocol::ChatCompletions,
//! )?;
//! let client = OpenAiClient::new(config)?;
//! let response = client.chat(ChatRequest {
//!     messages: vec![Message::user("Hello".to_string())],
//!     ..Default::default()
//! }).await?;
//! println!("{}", response.content().unwrap_or_default());
//! # Ok(())
//! # }
//! ```
//!
//! # Streaming
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//! use futures::StreamExt;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let config = LlmConfig::for_provider(
//!     "local",
//!     "http://127.0.0.1:11434/v1",
//!     "",
//!     "qwen3",
//!     LlmApiProtocol::ChatCompletions,
//! )?;
//! let client = OpenAiClient::new(config)?;
//! let mut stream = client.chat_stream(ChatRequest {
//!     messages: vec![Message::user("Tell me a joke".to_string())],
//!     ..Default::default()
//! }).await?;
//! while let Some(chunk) = stream.next().await {
//!     if let Some(content) = chunk?.delta.content {
//!         print!("{content}");
//!     }
//! }
//! # Ok(())
//! # }
//! ```

pub mod types {
    //! Canonical low-level LLM wire types.
    pub use echo_core::llm::types::*;
}

/// Provider-neutral prompt-cache contracts.
pub mod cache {
    pub use echo_core::llm::cache::*;
}

// Core traits from echo-core
pub use echo_core::llm::capabilities::{
    ModelProfile, ModelProfileOverride, ModelProfileResolver, ProviderCapabilities,
    ThinkingProfile, infer_context_window, resolve_thinking_profile,
};
pub use echo_core::llm::{
    ChatChunk, ChatRequest, ChatResponse, LlmApiProtocol, LlmClient, LlmTimeouts,
    ModelInputModality, SimpleChatOptions, ThinkingConfig, ThinkingLevel, ThinkingProtocol,
};

// Provider implementations and explicit runtime configuration.
pub use echo_integration::providers::{
    AnthropicClient, LlmConfig, OpenAiClient, ResponsesClient, resolve_protocol_endpoint,
};

pub use types::{JsonSchemaSpec, Message as LlmMessage, ResponseFormat, ToolDefinition};
