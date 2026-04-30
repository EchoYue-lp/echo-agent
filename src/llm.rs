//! LLM client façade — provider abstraction and chat APIs.
//!
//! Unified interface over multiple LLM providers. All clients implement the
//! [`LlmClient`] trait, providing both `chat()` and `chat_stream()` methods.
//!
//! # Supported Providers
//!
//! | Client | Backend | Feature |
//! |--------|---------|---------|
//! | [`OpenAiClient`] / [`DefaultLlmClient`] | OpenAI & compatible APIs | default |
//! | [`AnthropicClient`] | Native Claude API | `a2a` |
//! | [`OllamaClient`] | Local Ollama | default |
//! | [`ProviderFactory`] | Dynamic provider selection from config | default |
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let client = OpenAiClient::from_env("qwen3-max")?;
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
//! let client = OpenAiClient::from_env("qwen3-max")?;
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
    pub use echo_integration::providers::ollama::OllamaClient;
    pub use echo_integration::providers::openai::{DefaultLlmClient, OpenAiClient};
}

use futures::Stream;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

// Core traits from echo-core
pub use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};

// Provider implementations from echo_integration::providers
pub use echo_integration::providers::anthropic::AnthropicClient;
pub use echo_integration::providers::ollama::OllamaClient;
pub use echo_integration::providers::openai::{DefaultLlmClient, OpenAiClient};

// Config & Factory
pub use config::{LlmConfig, LlmProvider};
pub use echo_integration::providers::ProviderFactory;

// Wire types for internal use
#[allow(unused_imports)]
pub(crate) use types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Message,
};
pub use types::{JsonSchemaSpec, Message as LlmMessage, ResponseFormat, ToolDefinition};

/// Compatibility helper for assembling OpenAI-compatible request headers.
///
/// This remains available from the root facade so older call sites can stay on
/// `echo_agent::llm::*` while the canonical implementation lives in
/// `echo_integration::providers::openai`.
pub fn assemble_req_header(model: &config::ModelConfig) -> echo_core::error::Result<HeaderMap> {
    echo_integration::providers::openai::assemble_req_header(model)
}

/// Compatibility helper for a one-shot OpenAI-compatible chat completion call.
#[allow(clippy::too_many_arguments)]
pub async fn chat(
    client: Arc<Client>,
    model_name: &str,
    messages: &[Message],
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    stream: Option<bool>,
    tools: Option<Vec<ToolDefinition>>,
    tool_choice: Option<String>,
    response_format: Option<ResponseFormat>,
) -> echo_core::error::Result<ChatCompletionResponse> {
    echo_integration::providers::openai::chat(
        client,
        model_name,
        messages,
        temperature,
        max_tokens,
        stream,
        tools,
        tool_choice,
        response_format,
    )
    .await
}

/// Compatibility helper for OpenAI-compatible streaming chat completions.
#[allow(clippy::too_many_arguments)]
pub async fn stream_chat(
    client: Arc<Client>,
    model_name: &str,
    messages: Vec<Message>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    tools: Option<Vec<ToolDefinition>>,
    tool_choice: Option<String>,
    response_format: Option<ResponseFormat>,
    cancel_token: Option<CancellationToken>,
) -> echo_core::error::Result<
    impl Stream<Item = echo_core::error::Result<ChatCompletionChunk>> + use<>,
> {
    echo_integration::providers::openai::stream_chat(
        client,
        model_name,
        messages,
        temperature,
        max_tokens,
        tools,
        tool_choice,
        response_format,
        cancel_token,
    )
    .await
}
