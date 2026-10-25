# IM Channels —— 接入即时通讯平台

echo-agent 支持将 Agent 接入 QQ Bot、飞书等 IM 平台，通过插件化架构实现消息自动收发。个人微信原生 adapter 已加入当前源码候选；其编译、原生接入及真实平台验收仍待完成，限制见下文。

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

### 通过 owned Channel 前向调用

成功 start 后，`ChannelHandle::handle_message(message, cancel)` 调用原 entry
绑定的 handler 并返回 outbound；`reply_message(outbound, cancel)` await 同一
handler 的 reply，与既有 plugin.send 独立。`handle_message_stream(message,
cancel)` 返回 `ChannelMessageStream`，包含 owned
`BoxStream<'static, Result<OutboundMessage>>` 与同一次准入的 child cancellation
token，实现 Stream 并提供 into_parts。adapter 有自己的有界 send 缓冲时也监听该
精确 token，stop 才能解除满缓冲并阻止旧 item；调用方真实消费 items，框架不隐式
执行 reply。

每个 token 只属于当前 invocation。取消该调用或 drop waiter 不取消兄弟调用、也不
关闭整个 Session。stop 先关闭原 generation 准入并发取消，再停止 transport，等待
真实 invocation receipt 后关闭 handler；close 失败或被取消保留原 owner 供 retry。
未成功 start 的 handler 仅是 cleanup debt，不能接受消息；restart 后旧 captured
future 不能转向新 generation。

驱动器持有原 handler 和调用租约，因此流可以移出 manager borrow，而无需泄漏 guard
或做不安全生命周期转换。legacy 本地 future 可以在取消时 drop；声明
`settles_on_cancel` 的实现必须通过 `handle_with_cancel`、`reply_with_cancel`、
`handle_stream_with_cancel` 把 token 传给实际驱动并保留真实结算。取消通知不是外部
Run terminal 或 OS 进程清理证明。callback 可在释放外部 registry guard 后读 manager
metadata，同步等待自身 mutation/close 不属于支持目标。SDK wire 与语言合同在独立
SDK 仓库维护和验证，不由这些新 Rust API 自动扩大。

### 观察传输事实与替换已结算的 adapter

`start()` 成功只表示原 transport owner 已启动，`health_check()` 可能只检查 token／task，均不能证明连接成功。内置 `ChannelPlugin::transport_status()` 返回最新值 watch，第三方 plugin 默认可返回 `None`（Unknown）。QQ 仅在 READY／RESUMED、飞书长连接仅在真实 WS 握手、webhook 仅在本机 bind 成功时给 Ready。`LocalWebhookListener` 只证明本机监听，不代表平台可达或消息投递；错误先脱敏，发送 API 接受与连接状态、对方已读各是不同事实。

owned `ChannelHandle::transport_status().await` 从原 adapter 捕获 `{ mutation_epoch, receiver }`。控制操作后与 `management_snapshot().await` 的当前 epoch 核对，丢弃旧 epoch 观察；每次内置 start 换独立源。页面关闭或 drop watch 不停止服务，watch 关闭也不是成功停机收据。源已关闭时即使最后保留 Ready 也投影 Unknown，不改变原 handler／stop 事实。借用 trait 调用在 handle 生命周期锁忙时返回 None；需要完整控制观察时使用 owned 方法。

`management_snapshot()` 另外投影原 `handler_attached`、`active_invocations`、`shutdown_settled`。transport Stopped 但 handler close 失败仍未结算。`replace_stopped_plugin(new_plugin).await` 只接受已明确 stop、无 handler／活动 invocation、且 id／label／完整 capabilities 相同的原 entry。它 checked 递增原 epoch、使旧 stop 收据失效，仅替换 adapter；新 start 仍走原生命周期。stop／close 失败、旧 epoch 或计数耗尽都保留旧 entry；配置 revision 和控制串行仍归应用。

stop 等原 adapter 实际 task join，取消等待者不会丢 join owner。heartbeat 归同一原连接任务，webhook graceful shutdown 等已准入请求；pending handler 可令 stop 继续等待。webhook path 保留 Axum 0.7.9 的 capture／wildcard（如 `/events/:tenant`、`/events/*tail`），仅在构造前用同一已锁 matchit 0.7.3 parser 拒绝非法语法。QQ HELLO 的心跳间隔为 0 时返回明确协议错误，不为正间隔另设业务阈值。观察层不另建重试，也不强制退出。详见 [ADR 0092](../adr/0092-channel-control-observations.md)。这些 Rust API 不自动扩大独立 SDK 合同；本地 fixture 不代表真实平台验收。

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

入站消息带附件时，同一个 Turn 传入 typed user message：非空文本在前，附件按原顺序
排列。图像按字节签名识别 PNG、JPEG、GIF、WebP，并用正确 MIME 的 data URL 编码；
无法识别的图像在模型接纳前拒绝。文件以 base64 `ContentPart::File` 保留原始字节，
transport 未给文件名时使用稳定生成名。核心消息类型没有音频、视频对应的 typed part，
因此这两类附件明确报错，不会悄悄退化为纯文本。无附件消息沿用原纯文本路径。
这保证 framework typed 投影保留附件字节与部件顺序，不表示所有 transport 元数据
都有对应字段，也不保证各 provider 都能读取任意二进制文件；
provider adapter 可能把不支持的文件渲染成只有名称的占位内容。参见
[ADR 0077](../adr/0077-channel-attachment-projection.md)。

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
    pub channel_id: String,   // "qqbot" | "feishu" | "weixin"
    pub sender_id: String,    // 传输层命名空间化后的发送者规范标识
    pub chat_id: String,      // 会话标识
    pub chat_type: ChatType,  // Direct | Group
    pub text: String,
    pub message_id: String,   // 平台原始消息 ID（用于回复）
    pub timestamp: u64,
    pub attachments: Vec<MessageAttachment>,
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

## 个人微信（源码候选，待验收）

`echo_agent::channels::weixin` 提供原生 HTTPS/JSON 的 `WeixinApi`、
`QrChallenge`/`QrStatus`/`WeixinCredentials`、`WeixinChannel`、
`WeixinDeliveryHandle` 和 `WeixinContextRef`。协议依据是腾讯第一方
[Tencent/openclaw-weixin 874637a0，2.4.10](https://github.com/Tencent/openclaw-weixin/tree/874637a0dbc24050f72d1385add4fc6170a2d726)，
原项目为 [MIT 许可证](https://github.com/Tencent/openclaw-weixin/blob/874637a0dbc24050f72d1385add4fc6170a2d726/LICENSE)。
这里实现协议 adapter，不安装 OpenClaw Gateway，不注入桌面微信。固定客户端源码不能证明新客户端获准连接或实际投递成功。

平台能力限定为 direct 单聊文字。媒体、混合媒体输入及群聊不执行残余文本；当前给出拒绝诊断，不承诺附件理解、ASR 或联系人枚举。
账号与 peer 通过 `encode_target`/`decode_target` 形成可逆 canonical target；稳定账号身份、持久 credential generation 与进程实例身份分别保留。
注入既有 `Store` 只保存隔离的 cursor/context 协议状态，不创建消息 inbox、TaskRun store 或调度器。

QR client 区分等待、已扫码、数字码、受阻、过期、redirect、confirmed 和 already-bound。confirmed 不等于宿主保存成功，already-bound 不包含可冒充新登录的凭据。
GET/POST 使用各自 headers；HTTPS origin 校验及禁用自动重定向防止本机凭据被错误转发，错误不返回 raw URL/body。
宿主负责认证 operation、配置提交、允许的 peer 和 UI，框架不把 EKO 联系人策略写进通用权限系统。

`MessageHandler::bind_durable_ingress(service_cancel)` 与 `admit_with_ingress(msg, cancel, receipt) -> Result<()>` 是默认明确 Unsupported 的可选接缝。`SessionHandler::with_durable_ingress()` 声明同一 factory 的能力；原 session owner 在业务准入前向 inner 绑定同一 session child。inner 绑定失败仍保留原 owner/close 义务，不试跑 legacy、不假 ACK；活实例不能重绑，已关闭实例不能复活。QQ/飞书继续原接口。

| Disposition | 必须已有的原权威事实 |
| --- | --- |
| `DurableInput` | 完整输入/binding 已持久准入，或原 identity/digest 完全相同 |
| `ControlHandled` | 原控制效果已处理 |
| `TerminalRejected` | 明确终局拒绝且没有业务准入 |
| `ControlRecoveryRequired` | 原控制 journal 已持久保留效果不明及禁止重放义务；不是成功 |

返回、错误、publisher drop、lazy stream/EOF 或模型完成均不是 ACK。旧未发布 `handle_stream_with_ingress` 主路径已退出。admit 复用 host 原持久 owner、原 reply route publication 和 claim 唤醒，不返回响应流、不等 claim 槽/模型/HITL/最终输出，也不逐输入暗建输出 waiter。queued 只有 DATA；实际长 driver/renderer 由原 host 的执行许可和 close owner 持有，即时控制确认也使用同一原 sender。Framework 拦截的 durable reset 在现有 `OutboundMessage.reply_to` 保留原 provider message ID，交同一 inner host 解析并校验该条准确 context 关联；legacy scalar/stream reset 回复路径保持原行为。

微信在 accepting/spawn/poll 前绑定 transport service child。原 JoinSet 只拥有真实尚未退出的 admit 调用，沿原 **32 普通 + 8 控制**许可到实际返回才释放；ACK 提前到不释放调用许可，spawn 前回收已结束项。唯一原发送队列仍为 32，不保留逐输入 transport 响应任务。严格 batch cursor 只有全部原 receipt 到达才前进。在可真实持久准入时，普通前缀只积累原 DATA，后页 `/reply` 能到原 HITL owner；不宣称 DATA/索引/route 存储恒定内存，也不承诺磁盘/I/O 满载故障时控制永远可达。不能接收仍 Unknown/游标停，不把 Busy 假作终局拒绝。

回复固定原 account/peer/credential generation/context，沿 `ChannelSessionInstance::bind_outbound` 保存原 fence template；排队模板不提前领投递许可。`has_pending_work` 防 idle 误回收原 host 工作。Stop 父 token 先通过同 child 关闭 host claim，再等待准入、原 sender effect 和原 handler close；waiter 取消不丢 driver/renderer/恢复初始化实际句柄。child 取消不反向停止 transport 父。原 scalar/stream 路径保持行为。

`capture_message_context(target, message_id)` 读取原消息关联，`send_with_context` 将准确 Option 交同一 sender。恢复使用 `validate_message_context(target, message_id, expected)`：None 必须存在真实 `context: null` 关联；Some 必须精确 identity/reference 且原 token record 可读有效。缺关联/字段、身份/reference/token 漂移或不同凭据 generation 均拒绝，不发 HTTP、不借 latest、不修状态、不返回秘密。A2 持久 binding、never-claimed 证明、workspace generation 和 TaskRun 意图留原 host 策略。

typed send 仍等待唯一 producer 的真实 `Accepted`/`Rejected`/`Unknown`；HTTP200、入队、start 不等于成功，Unknown 不准重放。只有真实 authenticated 业务 poll 证明 `WeixinAuthenticatedPoll` Ready；`-14` 停止新收发并要求重新认证。

源码反例使用真实本地 HTTP poll/sender、SessionHandler 与通用文件持久 host：40 条输入、128 单消息页后控制、原长流分段、提前 ACK 持有调用许可及取消 Stop waiter 后的实际 join。Framework focused 已实际通过 135 条渠道用例（含全部 10 条新增或重命名 NoStream 用例）、12 条 facade 用例和 2 条仅编译 doctest；demo38 类型检查、匹配范围的 warning/panic lint、featureless 隔离及 fmt/check 同样通过。该受控夹具资格绑定未改变的 Rust 输入，不证明 host 生产持久化、完整合并矩阵、官网镜像、应用恢复、原生 GUI、真实扫码收发或精确 main CI。详见 [ADR 0099](../adr/0099-personal-weixin-channel.md)。

`demo38_im_channels` 继续演示 QQ/飞书。默认 `AgentChannelHandler` 没有持久 ingress opt-in；把它直接注册到微信或在 wrapper 中补一个假 ACK 会违反接收合同。示例的原编译链仍适用，不能把注释更新当作编译通过。

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

ChannelManager 在注册时固定插件的 ID、label 和 capabilities 声明。
即使第三方插件在 start/stop 中修改内部元数据，托管的 `get` / `get_mut` 仍返回
注册声明；修改声明需要新的注册生命周期。异步方法仍调用原插件实例。
持有 owned handle 不会使 `get_mut` 消失；可变借用会使先前 transport-stop 事实失效。

外部 async mutex 保护 manager 时，先用短 guard 取 owned 操作，释放 guard 后再 await：

```rust
let channel = {
    let guard = shared_manager.lock().await;
    guard.get_owned("qqbot")
};
if let Some(channel) = channel {
    channel.health_check().await?;
}
let shutdown = { shared_manager.lock().await.stop_all_owned() };
shutdown.await?;
```

`ChannelHandle` 保留同一原插件实例及清理责任。`start_all_owned` / `stop_all_owned`
在构造时捕获注册快照，之后注册的插件进入下一批；未轮询的 future 不调用插件或
handler factory。Start 在等待插件前保存 handler 责任；失败或取消后可重试。
Transport 成功停止后，重试只关闭未结算 handler；成功关闭不等于撤销注册。
Drop manager 不能代替 awaited shutdown。同一插件回调递归等待自身生命周期锁
不在保证范围内。详见 [ADR 0084](../adr/0084-channel-owned-operations.md)。

## 运行完整示例

```bash
# 配置环境变量后
cargo run -p echo-agent-learning --example demo38_im_channels --features channels
```
