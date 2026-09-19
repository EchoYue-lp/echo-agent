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
use std::sync::Arc;
use echo_agent::channels::{
    AgentChannelHandler, ChannelManager, ChannelSessionInstance, FeishuChannel, FeishuConfig,
    MessageHandler, QqChannel, QqConfig, SessionConfig, SessionHandler,
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

    // 单个通道可直接使用显式 provider 配置构造 handler。
    let _standalone_handler = AgentChannelHandler::from_config(
        AgentConfig::standard("gpt-5.5", "im-assistant", "请清晰回答用户。"),
        llm_config.clone(),
    )?;

    // Session factory 共享 provider transport，但每个会话中的每位发送者都保留
    // 独立 Agent 状态，群聊成员之间也不会共享上下文。
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
            move |instance: &ChannelSessionInstance| -> Box<dyn MessageHandler> {
                // timeout/reset 后不得恢复旧模型上下文时，可将该值纳入
                // 临时 runtime identity。
                let _runtime_incarnation = instance.incarnation_id();
                Box::new(AgentChannelHandler::from_config_with_client(
                    AgentConfig::standard("gpt-5.5", "im-assistant", "请清晰回答用户。"),
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

## ChannelPlugin 接口

`AgentChannelHandler` 将每条入站消息交给 `AgentTurnDriver`。标准 handler 只在
`TurnReceipt` 为 `Completed + Delivered` 且包含最终答案时生成回复；取消或失败
不能被误判为成功。框架调用者可通过
`AgentChannelHandler::drive_turn(&message, cancel_token)` 获取 Turn 身份、用量和
取消终态。这里的 delivery 指事件 sink 接纳，向 IM 平台发送则继续由 channel
generation fence 约束；`drive_turn_with_sink` 可接入调用方自己的 Journal 或投影
sink，默认 sink 仅表示进程内接纳，不是 QQ/飞书送达确认。framework session reset
会把取消 token 交给该 driven handler，并等待 Turn 结算后再确认替换。底层 Rust
`Agent::chat` API 保留。参见
[ADR 0046](../adr/0046-turn-execution-delivery-settlement.md)。

资源关闭是独立边界：`ChannelManager::stop_all` 先停止每个 transport，再 await
它保留的 `MessageHandler::close`。`SessionHandler` 取消 sender generation、
等待全部 accepted/polled active stream 与 delivery lease，然后 await 每个 sender
Agent close。shutdown 也等待不发布 driven terminal 的 legacy/custom handler；
该 receipt 只证明资源生命周期，不新建 Turn terminal。reset 继续使用既有
driven-only terminal settlement policy。
reset 或超时替换会先关闭旧 Agent，再发布新 Agent。关闭错误或调用方取消时保留
handler/session owner，后续可重试 `stop` 或 `close`，且不会重新开放消息接纳。
有资源的自定义 handler 应实现 `MessageHandler::close`；无资源 handler 使用默认
no-op。参见 [ADR 0066](../adr/0066-agent-adapter-close-ownership.md)。

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
    pub sender_id: String,    // 传输层命名空间化后的发送者规范标识
    pub chat_id: String,      // 会话标识
    pub chat_type: ChatType,  // Direct | Group
    pub text: String,
    pub message_id: String,   // 平台原始消息 ID（用于回复）
    pub timestamp: u64,
}
```

`SessionHandler` 使用 `channel_id + chat_id + sender_id` 作为 Agent 会话键。同一会话中，
同一发送者持续复用一个 handler；不同群聊成员则分别拥有独立的对话状态、执行锁、交互状态、
超时替换和 reset 生命周期。空的 channel、conversation、sender 标识和 `unknown` sender
哨兵都无法形成稳定的用户会话键，因此会被直接拒绝；带首尾空白的标识同样会被拒绝，而不是
被静默改写。内置传输不会把这类错误消息交给 Agent。飞书的两个身份命名空间会在查找会话前
规范为 `open_id:{value}` 或 `user_id:{value}`。

每次 factory 调用都会收到一个 `ChannelSessionInstance`。其中稳定的 channel、conversation、
sender 坐标用于标识产品会话，`incarnation_id()` 则只标识当前 handler 生命周期。同一个保留的
handler 持续复用 incarnation；framework timeout/reset 替换会创建新 incarnation；若 reset 由
应用自己编排，则可在旧工作完成结算后调用 `rotate()`，原子推进同一个 framework 权威。所有
clone 与 `SessionEndInfo` 都能看到该变化，因此应用可以精确回收旧模型/runtime context，同时
继续用稳定产品会话 ID 保存 journal 与 Task 历史。

reset 会先 retire 旧 generation 并取消其 stream。已经被内置 transport 接纳的 chunk 可以先完成
结算，reset 随后才会确认；旧 generation 的后续 chunk 会在进入队列或网络请求前被拒绝。
replacement handler 与 reset 回复只在该 delivery barrier 之后发布，因此 reset 确认之后不会再出现
旧回复。已经准入但从未 poll 的 stream 仍持有自己的 receipt，其 `SessionEndInfo` 清理回调会在
stream 被 drop 后执行。如果消费者回调自身 panic，`SessionHandler` 会在 lifecycle 边界内隔离
该 panic，不会让它从 stream 析构继续传播，也不会污染 replacement session。详见
[ADR 0057](../adr/0057-channel-generation-delivery-fence.md)。

自定义 Agent driver 应把稳定产品 ID 放在
`AgentInvocationContext.runtime.conversation_id`，并把 instance 派生的 runtime key 同时传给
`runtime_state_id` 与 `transcript_generation_id`。这样同一 incarnation 的 checkpoint load/save
保持对称，稳定 transcript 也能幂等追加，同时不会把旧产品历史重新注入模型。
即使消费者有意共享同一个 `ReactAgent`，context 仍按 value-scoped identity 隔离：
`runtime_state_id` 变化时，下一次模型请求前必须精确 reset/restore；只有相同 identity 才能复用
warm messages。运行时会在任何可取消的 restore 之前先发布 `Hydrating` 标记，并在该边界清空
rollback snapshots，因此被取消的切换不会让 partial context 或旧 snapshot 冒充另一 identity。

### OutboundMessage —— 发送的消息

```rust
pub struct OutboundMessage {
    pub channel_id: String,
    pub to: String,
    pub chat_type: ChatType,
    pub text: String,
    pub reply_to: Option<String>,  // 被回复的消息 ID
    // SessionHandler 会附加进程内 opaque delivery fence。
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

`QqConfig` 与 `FeishuConfig` 提供 credential-safe `Debug`：client/app secret、
Webhook verification token 和 signing key 都显示为 `[REDACTED]`。Channel
transport 日志和返回的诊断也会在格式化前脱敏符合凭据形态的响应文本与 URL；实际请求和
连接行为不变。

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
for started in manager.start_all(|_channel_id| {
    Arc::new(MyHandler::new(llm_client.clone()))
}).await {
    started.result?;
}

// 停止 transport、排空 handler，并等待各 sender Agent close。
manager.stop_all().await?;
```

## 运行完整示例

```bash
# 配置环境变量后
cargo run -p echo-agent-learning --example demo38_im_channels --features channels
```
