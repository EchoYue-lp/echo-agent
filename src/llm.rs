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

/// Direct re-exports from `echo_core::llm`.
pub mod core {
    pub use echo_core::llm::*;

    /// Canonical LLM wire types from `echo_core`.
    pub mod types {
        pub use echo_core::llm::types::*;
    }
}

/// Direct re-exports from `echo_integration::providers`.
pub mod integration {
    pub use echo_integration::providers::*;
}

pub mod types {
    //! Compatibility re-export of canonical wire types from `echo_core::llm::types`.
    pub use echo_core::llm::types::*;
}

pub mod config {
    //! Compatibility re-export of provider config from `echo_integration`.
    pub use echo_integration::providers::config::*;
}

pub mod providers {
    //! Compatibility re-export of provider implementations from `echo_integration`.
    pub use echo_integration::providers::anthropic::AnthropicClient;
    pub use echo_integration::providers::openai::OpenAiClient;
    pub use echo_integration::providers::responses::ResponsesClient;
}

// Core traits from echo-core
pub use echo_core::llm::capabilities::{
    ModelProfile, ModelProfileOverride, ModelProfileResolver, ProviderCapabilities,
};
pub use echo_core::llm::{
    ChatChunk, ChatRequest, ChatResponse, LlmApiProtocol, LlmClient, ModelInputModality,
    SimpleChatOptions, ThinkingConfig, ThinkingLevel, ThinkingProtocol,
};

// Provider implementations from echo_integration::providers
pub use echo_integration::providers::anthropic::AnthropicClient;
pub use echo_integration::providers::openai::OpenAiClient;
pub use echo_integration::providers::responses::ResponsesClient;

// Explicit runtime configuration
pub use config::{LlmConfig, resolve_protocol_endpoint};

pub use types::{JsonSchemaSpec, Message as LlmMessage, ResponseFormat, ToolDefinition};
