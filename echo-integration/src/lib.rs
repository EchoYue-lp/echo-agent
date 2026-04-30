//! # echo_integration
//!
//! Integration layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.
//!
//! ## Modules
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`providers`] | LLM clients: `OpenAiClient`, `AnthropicClient`, `OllamaClient`, `ProviderFactory` |
//! | [`mcp`] | MCP protocol: stdio, SSE, HTTP transports; `McpManager`, `McpServerConfig` |
//! | [`channels`] | IM integrations: QQ Bot (WebSocket) and Feishu (Webhook) |
//!
//! Most users should depend on `echo_agent` (the facade crate) instead of
//! depending on `echo_integration` directly.

pub mod channels;
pub mod mcp;
pub mod providers;
