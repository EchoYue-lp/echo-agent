# echo-integration

Integration layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Contents

- **LLM Providers**: OpenAI, Anthropic, DeepSeek, Qwen (DashScope), Ollama
- **MCP Protocol**: Model Context Protocol client/server (stdio, SSE, HTTP transports)
- **IM Channels**: QQ Bot (WebSocket) and Feishu (Webhook) integrations

## Usage

```rust
use echo_integration::providers::ProviderFactory;
use echo_integration::mcp::McpManager;
use echo_integration::channels::ChannelManager;
```

## License

MIT
