# echo-integration

[![crates.io](https://img.shields.io/crates/v/echo_integration?color=brightgreen)](https://crates.io/crates/echo_integration)
[![docs.rs](https://docs.rs/echo_integration/badge.svg)](https://docs.rs/echo_integration)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)

Integration layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Quickstart

```toml
[dependencies]
echo_integration = "0.1"
```

```rust
use echo_integration::providers::ProviderFactory;
use echo_integration::mcp::McpManager;
use echo_integration::channels::ChannelManager;

// Create an LLM provider
let provider = ProviderFactory::create_openai("gpt-4o", std::env::var("OPENAI_API_KEY")?)?;

// Connect to an MCP server
let mut mcp = McpManager::new();
mcp.connect_stdio("my-server", &["node", "./server.js"]).await?;
```

## Contents

- **LLM Providers**: OpenAI, Anthropic, DeepSeek, Qwen (DashScope), Ollama
- **MCP Protocol**: Model Context Protocol client/server (stdio, SSE, HTTP transports)
- **IM Channels**: QQ Bot (WebSocket) and Feishu (Webhook) integrations

## License

MIT
