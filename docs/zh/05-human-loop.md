# 人工介入（Human-in-the-Loop）

## 是什么

Human-in-the-Loop（HIL）是一种在 Agent 自动执行过程中插入人工决策点的机制。当 Agent 即将执行某个高风险操作时（如删除文件、发送邮件、转账），先暂停并向人类请求确认，再决定是否继续。

echo-agent 支持两种介入场景：

| 场景 | 说明 |
|------|------|
| **审批（Approval）** | 工具执行前弹出 y/n 确认，用户决定是否允许 |
| **输入（Input）** | Agent 需要额外信息时，向用户请求自由文本输入 |

---

## 解决什么问题

完全自动化的 Agent 存在风险：
- 执行不可逆操作（删除、发送、扣款）前没有确认
- 信息不足时凭猜测行动，而不是询问用户
- 生产环境中需要审计记录（谁批准了什么操作）

Human-in-the-Loop 在自动化效率与人工安全之间取得平衡。

---

## 三种 Provider

### ConsoleHumanLoopProvider（命令行，默认）

```rust
// Agent 执行时会在控制台打印：
// 工具 [delete_file] 需要人工审批，是否批准执行？(y/n)
// 用户输入 y → 执行   n → 跳过
```

### WebhookHumanLoopProvider（HTTP 回调）

将审批请求发送到外部 HTTP 服务，等待服务返回决策。适合：
- 企业审批系统集成（钉钉、企微机器人）
- 将审批推送到外部工单系统

```rust
use echo_agent::prelude::*;

let provider = WebhookHumanLoopProvider::new(
    "https://your-approval-service/approve",
    30, // 超时秒数
);
agent.set_approval_provider(Arc::new(provider));
```

### WebSocketHumanLoopProvider（WebSocket 推送）

在本地启动 WebSocket 服务器，将审批请求实时推送给已连接的客户端（前端 UI）。适合：
- 带可视化界面的 Agent 应用
- 移动端 App 接收审批通知

```rust
use echo_agent::prelude::*;

let provider = WebSocketHumanLoopProvider::new("127.0.0.1:9000").await?;
agent.set_approval_provider(Arc::new(provider));
```

---

## 使用方式

### 工具审批：`add_need_appeal_tool`

标记某个工具为"需要审批"，在执行前自动弹出人工确认：

```rust
use echo_agent::prelude::*;
use echo_agent::tools::shell::ShellTool;

let config = AgentConfig::new("qwen3-max", "agent", "你是一个系统管理助手")
    .enable_tool(true)
    .enable_human_in_loop(true);

let mut agent = ReactAgent::new(config);

// 注册工具为"需要审批"：执行前必须得到用户确认
agent.add_need_appeal_tool(Box::new(ShellTool));

let answer = agent.execute("删除 /tmp 下所有 .log 文件").await?;
```

执行时控制台显示：
```
🔔 工具 [shell] 需要人工审批
   参数: {"command": "rm /tmp/*.log"}
   是否批准执行？(y/n): _
```

---

### 文本输入：`human_in_loop` 工具

当 Agent 信息不足时，主动向用户请求输入。通过注册 `HumanInLoop` 工具实现（`enable_human_in_loop=true` 时自动注册）：

```rust
// Agent 系统提示词中告知 LLM 何时使用 human_in_loop 工具：
let system = "当你需要额外信息才能完成任务时，使用 human_in_loop 工具向用户提问。";

let config = AgentConfig::new("qwen3-max", "agent", system)
    .enable_tool(true)
    .enable_human_in_loop(true);

let mut agent = ReactAgent::new(config);
let answer = agent.execute("帮我订一张机票").await?;
// Agent 会调用 human_in_loop("请问您想去哪个城市？出发日期是？")
// 控制台等待用户输入后继续执行
```

---

## 自定义 Provider

实现 `HumanLoopProvider` trait 可接入任意审批系统：

```rust
use echo_agent::prelude::*;
use async_trait::async_trait;

struct SlackApprovalProvider;

#[async_trait]
impl HumanLoopProvider for SlackApprovalProvider {
    async fn request(&self, req: HumanLoopRequest) -> echo_agent::error::Result<HumanLoopResponse> {
        // 向 Slack 频道发送消息，等待 reaction 或回复
        let approved = send_slack_and_wait(&req.prompt).await;
        if approved {
            Ok(HumanLoopResponse::Approved)
        } else {
            Ok(HumanLoopResponse::Rejected { reason: Some("Slack 用户拒绝".to_string()) })
        }
    }
}

// fn send_slack_and_wait(...) -> bool { ... }
```

---

## 执行流程

```
Agent 准备执行工具 "delete_file"
    │
    ├─ 检查：HumanApprovalManager.needs_approval("delete_file") ?
    │
    ├─ 是 → 调用 approval_provider.request(HumanLoopRequest::approval(...))
    │         │
    │         ├─ Console: 等待用户在终端输入 y/n
    │         ├─ Webhook: POST 到外部服务，轮询结果
    │         └─ WebSocket: 推送给客户端，等待回调
    │
    ├─ Approved  → 继续执行工具
    └─ Rejected  → 将拒绝原因作为 tool result 返回给 LLM（LLM 可调整策略）
       Timeout   → 默认视为拒绝
```

对应示例：`examples/demo03_approval.rs`
