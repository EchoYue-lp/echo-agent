# IM Channels —— Integrating with Messaging Platforms

echo-agent supports connecting your Agent to mainstream IM platforms (QQ Bot, Feishu, etc.) through a pluggable architecture for automatic messaging.

## Architecture Overview

```
┌──────────────┐    ┌──────────────┐
│   QQ Bot     │    │    Feishu    │
│ (WebSocket)  │    │ (HTTP Webhook)│
└──────┬───────┘    └──────┬───────┘
       │                   │
       ▼                   ▼
┌──────────────────────────────────┐
│       ChannelManager             │
│  ┌────────┐  ┌────────┐         │
│  │ QQ     │  │ Feishu │         │
│  └───┬────┘  └───┬────┘         │
└──────┼───────────┼──────────────┘
       │           │
       ▼           ▼
┌──────────────────────────────────┐
│       ChannelHandler             │
│  IM message → Agent → auto reply  │
└──────────────────────────────────┘
       │
       ▼
┌──────────────────────────────────┐
│       Agent Core (ReAct)          │
│  Tools / Memory / Skills / MCP    │
└──────────────────────────────────┘
```

## Quick Start

### 1. Add Dependencies

```toml
[dependencies]
echo_agent = { path = "echo-agent", features = ["channels"] }
```

### 2. Configure Environment Variables

```bash
# QQ Bot
export QQ_APP_ID="your-qq-app-id"
export QQ_CLIENT_SECRET="your-qq-client-secret"

# Feishu
export FEISHU_APP_ID="your-feishu-app-id"
export FEISHU_APP_SECRET="your-feishu-app-secret"
export FEISHU_WEBHOOK_BIND="0.0.0.0:8080"

# LLM
export OPENAI_API_KEY="your-api-key"
export OPENAI_BASE_URL="https://dashscope.aliyuncs.com/compatible-mode/v1"
```

### 3. Launch

```rust
use std::sync::Arc;
use echo_agent::channels::{
    AgentChannelHandler, ChannelManager, FeishuChannel, FeishuConfig, MessageHandler,
    QqChannel, QqConfig, SessionConfig, SessionHandler,
};
use echo_agent::prelude::{AgentConfig, LlmApiProtocol, LlmClient, LlmConfig};

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
        echo_agent::error::ReactError::Other(
            "OPENAI_API_KEY is required for the IM channel provider".to_string(),
        )
    })?;
    let llm_config = LlmConfig::for_provider(
        "openai",
        std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
        api_key,
        "gpt-5.5",
        LlmApiProtocol::Responses,
    )?;

    // For one standalone channel, construct the handler directly.
    let _standalone_handler = AgentChannelHandler::from_config(
        AgentConfig::standard("gpt-5.5", "im-assistant", "Answer clearly."),
        llm_config.clone(),
    )?;

    // Session factories share one provider transport while each sender in each
    // conversation keeps independent Agent state, including in group chats.
    let llm_client: Arc<dyn LlmClient> = Arc::from(llm_config.build_client()?);

    let mut manager = ChannelManager::new();

    if let (Ok(app_id), Ok(secret)) = (
        std::env::var("QQ_APP_ID"),
        std::env::var("QQ_CLIENT_SECRET"),
    ) {
        manager.register(Box::new(QqChannel::new(QqConfig::new(app_id, secret))?))?;
    }
    if let (Ok(app_id), Ok(secret)) = (
        std::env::var("FEISHU_APP_ID"),
        std::env::var("FEISHU_APP_SECRET"),
    ) {
        manager.register(Box::new(FeishuChannel::new(FeishuConfig::new_long_poll(
            app_id, secret,
        ))?))?;
    }

    let session_config = SessionConfig::default().with_timeout_minutes(60);
    let handler_factory = move |_channel_id: &str| -> Arc<dyn MessageHandler> {
        let llm_client = Arc::clone(&llm_client);
        Arc::new(SessionHandler::new(
            session_config.clone(),
            move || -> Box<dyn MessageHandler> {
                Box::new(AgentChannelHandler::from_config_with_client(
                    AgentConfig::standard("gpt-5.5", "im-assistant", "Answer clearly."),
                    Arc::clone(&llm_client),
                ))
            },
        ))
    };
    for started in manager.start_all(handler_factory).await {
        started.result?;
    }

    tokio::signal::ctrl_c().await.ok();
    manager.stop_all().await
}
```

## ChannelPlugin Interface

All IM channels implement a unified interface:

```rust
#[async_trait]
pub trait ChannelPlugin: Send + Sync {
    /// Unique channel ID
    fn id(&self) -> &str;
    /// User-visible label
    fn label(&self) -> &str;
    /// Capability description
    fn capabilities(&self) -> &ChannelCapabilities;
    /// Start the channel
    async fn start(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()>;
    /// Stop the channel
    async fn stop(&mut self) -> Result<()>;
    /// Send a message
    async fn send(&self, msg: OutboundMessage) -> Result<()>;
}
```

## Message Flow

### InboundMessage —— Received Messages

```rust
pub struct InboundMessage {
    pub channel_id: String,   // "qqbot" | "feishu"
    pub sender_id: String,    // Canonical transport-scoped sender identity
    pub chat_id: String,      // Session identifier
    pub chat_type: ChatType,  // Direct | Group
    pub text: String,
    pub message_id: String,   // Platform message ID (for replies)
    pub timestamp: u64,
}
```

`SessionHandler` keys an Agent session by `channel_id + chat_id + sender_id`.
Messages from the same sender in the same conversation reuse one handler, while
different group members have independent conversation state, execution locks,
interaction state, timeout replacement, and reset lifecycle. Empty channel,
conversation, or sender identifiers and the `unknown` sender sentinel are
rejected because they cannot form a stable per-user session key. Identifiers
with surrounding whitespace are also rejected instead of being silently
rewritten. Built-in transports never forward these malformed messages to an
Agent. Feishu normalizes its two identity namespaces to `open_id:{value}` or
`user_id:{value}` before session lookup.

### OutboundMessage —— Sent Messages

```rust
pub struct OutboundMessage {
    pub channel_id: String,
    pub to: String,
    pub chat_type: ChatType,
    pub text: String,
    pub reply_to: Option<String>,  // Replied message ID
}
```

### Custom MessageHandler

```rust
struct MyHandler {
    llm_client: Arc<dyn LlmClient>,
}

#[async_trait::async_trait]
impl MessageHandler for MyHandler {
    async fn handle(&self, msg: InboundMessage)
        -> echo_agent::error::Result<OutboundMessage>
    {
        let mut agent = ReactAgentBuilder::new()
            .model("qwen3-max")
            .system_prompt("You are a helpful assistant")
            .enable_tools()
            .llm_client(self.llm_client.clone())
            .build()?;

        let reply = agent.chat(&msg.text).await?;

        Ok(OutboundMessage::new(
            &msg.channel_id, msg.reply_target(),
            msg.chat_type, &reply,
        ))
    }

    async fn reply(&self, _msg: OutboundMessage)
        -> echo_agent::error::Result<()>
    {
        // Channel auto-sends via send_tx, no manual handling needed
        Ok(())
    }
}
```

## QQ Bot

### Connection Method

QQ Bot uses **WebSocket Gateway** to receive messages and **HTTP API** to send messages.

```
Client ──WS Connection──▶ QQ Gateway ◀──HTTP Send── Client
```

### Configuration

```rust
pub struct QqConfig {
    pub app_id: String,
    pub client_secret: String,
}
```

### Supported Events

| Event | Description |
|-------|-------------|
| `C2C_MESSAGE_CREATE` | Direct message |
| `GROUP_AT_MESSAGE_CREATE` | Group @message |

### Token Management

Tokens are automatically cached and refreshed 5 minutes before expiration.

## Feishu

### Connection Method

Feishu uses **HTTP Webhook** to receive event pushes and **HTTP API** to send messages.

```
Feishu Server ──HTTP POST──▶ Your Webhook Server
Your Webhook Server ──HTTP POST──▶ Feishu API
```

### Configuration

```rust
pub struct FeishuConfig {
    pub app_id: String,
    pub app_secret: String,
    pub webhook_bind: String,       // e.g. "0.0.0.0:8080"
    pub webhook_path: String,       // e.g. "/webhook"
    pub verification_token: Option<String>,
}
```

### Webhook Deployment

1. After starting the app, configure `http://your-public-ip:port/webhook` in Feishu Open Platform
2. Feishu will send a challenge verification request, which is automatically responded to
3. Events are received after verification

### Supported Events

| Event | Description |
|-------|-------------|
| `im.message.receive_v1` | Receive message (text only) |

## ChannelManager

Manages the lifecycle of multiple channels:

```rust
let mut manager = ChannelManager::new();
manager.register(Box::new(QqChannel::new(config)?));
manager.register(Box::new(FeishuChannel::new(config)?));

// Create independent Handler for each channel
manager.start_all(|channel_id| {
    Arc::new(MyHandler::new(llm_client.clone()))
}).await?;

// Stop all
manager.stop_all().await?;
```

## Running the Full Example

```bash
# After configuring environment variables
cargo run --example demo38_im_channels --features channels
```
