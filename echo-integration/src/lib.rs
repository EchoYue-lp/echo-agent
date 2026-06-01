//! # echo_integration
//!
//! Integration layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.
//!
//! ## Modules
//!
//! | Module | Description | Feature |
//! |--------|-------------|---------|
//! | [`providers`] | LLM clients: `OpenAiClient`, `AnthropicClient`, `OllamaClient`, `ProviderFactory` | default |
//! | [`mcp`] | MCP protocol: stdio, SSE, HTTP transports; `McpManager`, `McpServerConfig` | `mcp` |
//! | [`channels`] | IM integrations: QQ Bot (WebSocket) and Feishu (Webhook) | `channels` |
//!
//! Most users should depend on `echo_agent` (the facade crate) instead of
//! depending on `echo_integration` directly.

#[cfg(feature = "mcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "mcp")))]
pub mod mcp;

#[cfg(feature = "lsp")]
#[cfg_attr(docsrs, doc(cfg(feature = "lsp")))]
pub mod lsp;

#[cfg(feature = "channels")]
#[cfg_attr(docsrs, doc(cfg(feature = "channels")))]
pub mod channels;

pub mod providers;
