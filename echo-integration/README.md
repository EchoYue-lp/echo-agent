# echo-integration

[![crates.io](https://img.shields.io/crates/v/echo_integration?color=brightgreen)](https://crates.io/crates/echo_integration)
[![docs.rs](https://docs.rs/echo_integration/badge.svg)](https://docs.rs/echo_integration)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)

Integration layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Quickstart

```toml
[dependencies]
echo_integration = "0.2"
```

```rust,no_run
use echo_integration::providers::{LlmConfig, ProviderFactory};

# fn build() -> echo_core::error::Result<()> {
let config = LlmConfig::openai(std::env::var("OPENAI_API_KEY")?, "gpt-5.5");
let _provider = ProviderFactory::from_config(&config)?;
# Ok(())
# }
```

## Contents

- **LLM Providers**: OpenAI, Anthropic, DeepSeek, Qwen (DashScope), Moonshot, Zhipu
- **MCP Protocol**: Model Context Protocol client/server (stdio, SSE, HTTP transports)
- **IM Channels**: QQ Bot (WebSocket) and Feishu (Webhook) integrations

## License

MIT
