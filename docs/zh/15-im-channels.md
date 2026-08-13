# IM Channels —— 接入即时通讯平台

echo-agent 支持将 Agent 接入主流 IM 平台（QQ Bot、飞书等），通过插件化架构实现消息自动收发。

## 架构概览

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
│  IM 消息 → Agent → 回复自动发回    │
└──────────────────────────────────┘
       │
       ▼
┌──────────────────────────────────┐
│       Agent 核心 (ReAct)          │
│  工具调用 / 记忆 / 技能 / MCP     │
└──────────────────────────────────┘
```

## 快速开始

### 1. 添加依赖

```toml
[dependencies]
echo_agent = { path = "echo-agent", features = ["channels"] }
echo_channels = { path = "echo-agent/echo-channels" }
```

### 2. 配置环境变量

```bash
# QQ Bot
export QQ_APP_ID="your-qq-app-id"
export QQ_CLIENT_SECRET="your-qq-client-secret"

# 飞书
export FEISHU_APP_ID="your-feishu-app-id"
export FEISHU_APP_SECRET="your-feishu-app-secret"
export FEISHU_WEBHOOK_BIND="0.0.0.0:8080"

# LLM
export OPENAI_API_KEY="your-api-key"
export OPENAI_BASE_URL="https://dashscope.aliyuncs.com/compatible-mode/v1"
```

### 3. 启动

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

    // 注册 QQ Bot
    let qq_config = QqConfig {
        app_id: std::env::var("QQ_APP_ID")?,
        client_secret: std::env::var("QQ_CLIENT_SECRET")?,
    };
    manager.register(Box::new(QqChannel::new(qq_config)?));

    // 注册飞书
    let feishu_config = FeishuConfig {
        app_id: std::env::var("FEISHU_APP_ID")?,
        app_secret: std::env::var("FEISHU_APP_SECRET")?,
        webhook_bind: "0.0.0.0:8080".to_string(),
        webhook_path: "/webhook".to_string(),
        verification_token: None,
    };
    manager.register(Box::new(FeishuChannel::new(feishu_config)?));

    // 启动所有通道
    let llm_ref = llm_client.clone();
    let handler_factory = move |_id: &str| -> Arc<dyn MessageHandler> {
        Arc::new(MyHandler::new(llm_ref.clone()))
    };
    manager.start_all(handler_factory).await?;

    // 等待 Ctrl+C
    tokio::signal::ctrl_c().await.ok();
    manager.stop_all().await?;

    Ok(())
}
```

## ChannelPlugin 接口

所有 IM 通道实现统一接口：

```rust
#[async_trait]
pub trait ChannelPlugin: Send + Sync {
    /// 通道唯一 ID
    fn id(&self) -> &str;
    /// 用户可见名称
    fn label(&self) -> &str;
    /// 能力描述
    fn capabilities(&self) -> &ChannelCapabilities;
    /// 启动通道
    async fn start(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()>;
    /// 停止通道
    async fn stop(&mut self) -> Result<()>;
    /// 发送消息
    async fn send(&self, msg: OutboundMessage) -> Result<()>;
}
```

## 消息流转

### InboundMessage —— 接收的消息

```rust
pub struct InboundMessage {
    pub channel_id: String,   // "qqbot" | "feishu"
    pub sender_id: String,    // 发送者标识
    pub chat_id: String,      // 会话标识
    pub chat_type: ChatType,  // Direct | Group
    pub text: String,
    pub message_id: String,   // 平台原始消息 ID（用于回复）
    pub timestamp: u64,
}
```

### OutboundMessage —— 发送的消息

```rust
pub struct OutboundMessage {
    pub channel_id: String,
    pub to: String,
    pub chat_type: ChatType,
    pub text: String,
    pub reply_to: Option<String>,  // 被回复的消息 ID
}
```

### 自定义 MessageHandler

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
            .system_prompt("你是一个友好的助手")
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
        // Channel 自动通过 send_tx 发送，无需手动处理
        Ok(())
    }
}
```

## QQ Bot

### 连接方式

QQ Bot 使用 **WebSocket Gateway** 接收消息，HTTP API 发送消息。

```
客户端 ──WS 连接──▶ QQ Gateway ◀──HTTP 发送── 客户端
```

### 配置

```rust
pub struct QqConfig {
    pub app_id: String,
    pub client_secret: String,
}
```

### 支持的事件

| 事件 | 说明 |
|------|------|
| `C2C_MESSAGE_CREATE` | 私聊消息 |
| `GROUP_AT_MESSAGE_CREATE` | 群聊 @消息 |

### Token 管理

Token 自动缓存，提前 5 分钟刷新，无需手动管理。

## 飞书

### 连接方式

飞书使用 **HTTP Webhook** 接收事件推送，HTTP API 发送消息。

```
飞书服务器 ──HTTP POST──▶ 你的 Webhook 服务器
你的 Webhook 服务器 ──HTTP POST──▶ 飞书 API
```

### 配置

```rust
pub struct FeishuConfig {
    pub app_id: String,
    pub app_secret: String,
    pub webhook_bind: String,       // 如 "0.0.0.0:8080"
    pub webhook_path: String,       // 如 "/webhook"
    pub verification_token: Option<String>,
}
```

### Webhook 部署

1. 启动应用后，将 `http://你的公网IP:端口/webhook` 配置到飞书开放平台
2. 飞书会发送 challenge 验证请求，自动响应
3. 验证通过后开始接收事件

### 支持的事件

| 事件 | 说明 |
|------|------|
| `im.message.receive_v1` | 接收消息（仅文本） |

## ChannelManager

管理多个通道的生命周期：

```rust
let mut manager = ChannelManager::new();
manager.register(Box::new(QqChannel::new(config)?));
manager.register(Box::new(FeishuChannel::new(config)?));

// 为每个通道创建独立的 Handler
manager.start_all(|channel_id| {
    Arc::new(MyHandler::new(llm_client.clone()))
}).await?;

// 停止所有
manager.stop_all().await?;
```

## 运行完整示例

```bash
# 配置环境变量后
cargo run --example demo38_im_channels --features channels
```
