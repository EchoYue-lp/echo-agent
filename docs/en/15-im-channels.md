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
echo_channels = { path = "echo-agent/echo-channels" }
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
use echo_agent::agent::Agent;
use echo_agent::llm::LlmClient;
use echo_agent::prelude::*;
use echo_channels::prelude::*;
use echo_providers::LlmConfig;
use std::sync::Arc;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let llm_client = create_llm_client()?;

    let mut manager = ChannelManager::new();

    // Register QQ Bot
    let qq_config = QqConfig {
        app_id: std::env::var("QQ_APP_ID")?,
        client_secret: std::env::var("QQ_CLIENT_SECRET")?,
    };
    manager.register(Box::new(QqChannel::new(qq_config)?));

    // Register Feishu
    let feishu_config = FeishuConfig {
        app_id: std::env::var("FEISHU_APP_ID")?,
        app_secret: std::env::var("FEISHU_APP_SECRET")?,
        webhook_bind: "0.0.0.0:8080".to_string(),
        webhook_path: "/webhook".to_string(),
        verification_token: None,
    };
    manager.register(Box::new(FeishuChannel::new(feishu_config)?));

    // Start all channels
    let llm_ref = llm_client.clone();
    let handler_factory = move |_id: &str| -> Arc<dyn MessageHandler> {
        Arc::new(MyHandler::new(llm_ref.clone()))
    };
    manager.start_all(handler_factory).await?;

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await.ok();
    manager.stop_all().await?;

    Ok(())
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
    pub sender_id: String,    // Sender identifier
    pub chat_id: String,      // Session identifier
    pub chat_type: ChatType,  // Direct | Group
    pub text: String,
    pub message_id: String,   // Platform message ID (for replies)
    pub timestamp: u64,
}
```

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
