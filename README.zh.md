<div align="center">

# 🚀 echo-agent

**为 Rust 打造的高速 AI Agent 框架 - 零成本抽象 • 内存安全 • 异步原生**

[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-1.3.0-brightgreen)](https://github.com/EchoYue-lp/echo-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![OpenAI Compatible](https://img.shields.io/badge/API-OpenAI%20兼容-green)](https://platform.openai.com/docs/api-reference)
[![Async](https://img.shields.io/badge/async-tokio-blue)](https://tokio.rs/)
[![Examples](https://img.shields.io/badge/examples-40%2B-blue)](./examples/)

**内存安全 • 零成本抽象 • 异步原生 • 生产就绪**

[English](./README.md) · [文档中心](docs/zh/README.md) · [示例](./examples/)

</div>

---

## 为什么选择 echo-agent？

绝大多数 AI Agent 框架基于 Python 构建。**echo-agent** 将完整的现代 Agent 框架能力带入 Rust 生态——与 LangGraph、CrewAI、AutoGen 功能对齐，同时提供只有 Rust 才能带来的性能和可靠性。

```rust
use echo_agent::prelude::*;
use echo_agent::{agent, tool};

#[tool(name = "add", description = "两数相加")]
async fn add(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a + b)))
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut agent = agent! {
        model: "qwen3-max",
        system_prompt: "你是一个计算助手",
        tools: [AddTool],
    }?;

    let answer = agent.execute("计算 1337 × 42").await?;
    println!("{answer}");
    Ok(())
}
```

**更进一步**——只需几行代码，将 Agent 部署到 QQ、飞书等 IM 平台：

```rust
let mut manager = ChannelManager::new();
manager.register(Box::new(QqChannel::new(qq_config)?));
manager.register(Box::new(FeishuChannel::new(feishu_config)?));
manager.start_all(handler).await?;  // 完成 —— 你的 Agent 已在 IM 中运行
```

---

## 亮点

- **40+ 项能力** —— ReAct 循环、工具、记忆、流式、多 Agent、技能、MCP、IM 通道、护栏、审计等
- **40 个可运行示例** —— 每个功能都有对应 demo，`cargo run` 即可体验
- **350+ 单元测试** —— 覆盖所有模块
- **6 个 crate，1 行导入** —— 模块化 workspace，但 `use echo_agent::prelude::*` 就够了
- **多模态支持** —— 文本、图片（base64 / URL）、文件附件可在同一条消息中混合使用
- **IM 平台接入** —— QQ Bot（WebSocket）和飞书（Webhook）开箱即用
- **声明式工作流** —— 用 YAML/JSON 定义 Agent 图，无需写 Rust 代码
- **统一重试** —— 一套 `RetryPolicy` 统管所有外部调用（LLM、MCP、A2A、沙箱）

---

## 🏗️ 架构概览

```
┌─────────────────────────────────────────────────────────┐
│                   用户 / 应用程序                        │
└────────────────────────┬────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────┐
│                    ReactAgent                            │
│  ┌──────────────┐  ┌────────────┐  ┌─────────────────┐  │
│  │ContextManager│  │ToolManager │  │  SkillManager   │  │
│  │(压缩)        │  │(执行)      │  │ (技能元数据)    │  │
│  └──────────────┘  └────────────┘  └─────────────────┘  │
│                                                         │
│  ┌──────────────┐  ┌────────────┐  ┌─────────────────┐  │
│  │  Checkpointer│  │   Store    │  │HumanApprovalMgr │  │
│  │(会话历史)    │  │(长期记忆)  │  │ (审批门)        │  │
│  └──────────────┘  └────────────┘  └─────────────────┘  │
│                                                         │
│  ┌──────────────────────────────────────────────────┐   │
│  │            子代理注册表                           │   │
│  │  { "数学代理": Arc<AsyncMutex<Box<dyn Agent>>>   │   │
│  │    "写作代理": ... }                              │   │
│  └──────────────────────────────────────────────────┘   │
└────────────────────────┬────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────┐
│                  LLM 提供方                              │
│        (OpenAI / DeepSeek / Qwen / Ollama / ...)         │
└─────────────────────────────────────────────────────────┘
```

---

## 功能一览

| 能力 | 描述 |
|------|------|
| 🔄 **ReAct 引擎** | Thought → Action → Observation 循环，内置 Chain-of-Thought |
| 🔧 **工具系统** | `#[tool]` 宏 + 自动 JSON Schema；超时、重试、并行执行 |
| 🧠 **双层记忆** | `Store`（长期 KV）+ `Checkpointer`（会话历史） |
| 🔍 **记忆工具** | `with_memory_tools(store)` —— 自动注入 remember / recall / search / forget |
| 🖼️ **多模态** | `ContentPart::Text` / `ImageUrl` / `File` —— 消息中嵌入图片与文件 |
| 📦 **上下文压缩** | 滑动窗口 / LLM 摘要 / 混合管道 |
| 📏 **Token 预算** | `max_tool_output_tokens` 自动截断 + think() 前主动触发压缩 |
| 🔁 **统一重试** | `RetryPolicy` + `with_retry` / `with_retry_if` 包装所有外部调用 |
| 🔀 **动态工具** | `remove_tool()` / `replace_tool()` —— 对话过程中动态调整工具集 |
| 🤝 **人工介入** | 工具审批门，支持命令行、Webhook、WebSocket |
| 🏗️ **多 Agent 编排** | Orchestrator → SubAgent 分派；Agent 间 Handoff |
| 💡 **Skill 系统** | 渐进式披露：发现 → 激活 → 使用 |
| 🔌 **MCP 协议** | 接入任意 MCP 服务（stdio / SSE / HTTP） |
| 📊 **Plan-and-Execute** | Planner + Executor：先规划后执行策略 |
| 📡 **流式输出** | `execute_stream()` 返回实时 `AgentEvent` 流 |
| 🌊 **工作流流式** | `Graph::run_stream()` —— 逐节点发出 `WorkflowEvent` 事件 |
| 📐 **结构化输出** | `extract::<T>()` —— 通过 JSON Schema 将 LLM 输出反序列化为 Rust 类型 |
| 📝 **声明式工作流** | `Graph::from_yaml()` / `from_json()` —— 零代码工作流定义 |
| 🛡️ **护栏系统** | 规则 / LLM 双模式内容过滤 |
| 🔑 **权限模型** | 声明式工具权限 + 可插拔策略 |
| 📋 **审计日志** | 结构化事件日志，可插拔存储后端 |
| 📈 **OpenTelemetry** | 分布式追踪与指标（OTLP） |
| 🎣 **宏系统** | `#[tool]`、`#[callback]`、`#[guard]`、`#[handler]`、`agent!{}`、`messages![]` |
| 🌐 **A2A 协议** | Agent Card 发布 / 跨框架协作 |
| 🏖️ **沙箱执行** | Local / Docker / K8s 代码执行，支持资源限制 |
| 🔗 **图工作流** | 有向图：线性、条件分支、循环、并行 fan-out/fan-in |
| 💬 **IM 通道** | QQ Bot（WebSocket）和飞书（Webhook）—— 将 Agent 接入即时通讯 |

---

## v1.2.0 新功能

### IM 通道集成

将你的 Agent 接入真实消息平台：

```rust
// QQ Bot —— WebSocket Gateway
let qq = QqChannel::new(QqConfig {
    app_id, client_secret,
})?;

// 飞书 —— HTTP Webhook
let feishu = FeishuChannel::new(FeishuConfig {
    app_id, app_secret,
    webhook_bind: "0.0.0.0:8080",
    webhook_path: "/webhook",
    verification_token: None,
})?;

let mut manager = ChannelManager::new();
manager.register(Box::new(qq));
manager.register(Box::new(feishu));
manager.start_all(handler).await?;
```

特性：
- **统一 `ChannelPlugin` 接口** —— 实现一个 trait 即可接入新平台
- **自动 Token 管理** —— OAuth 缓存 + 刷新，无需手动处理
- **WebSocket 自动重连** —— 指数退避，永不断线
- **消息队列** —— 异步 `mpsc` 通道，高负载下不丢消息
- **白名单支持** —— `ChatConfig::with_allow_from()` 实现访问控制

---

## v1.1.0 新功能

### Agent 目录重组 + StateGraph DSL

Agent 模块目录重组：`src/agent/react_agent` → `src/agents/react`，`src/plan_execute` → `src/agents/plan_execute`。新增 `agents/self_reflection` 自反思模块（Composite/Critic/LlmCritic）。StateGraph DSL 声明式工作流定义。Task 系统增强：DAG 调度、Hooks、Events、Store 持久化、Executor。

### Human-in-the-Loop 7 阶段管线

完整权限系统（对标 Claude Code）：

```
Bypass → Plan → Rules(deny-first) → ProtectedPaths → Cache(TTL) → DenialTracker → Mode dispatch
```

- **SessionApprovalCache** 带 TTL 过期（默认 30 分钟）
- **审计追踪**：`PermissionAuditSink` trait + InMemory/Logging/Composite 实现
- **ProtectedPathChecker**：`.git`/`.env`/`.ssh` 始终受保护
- **AI 分类器**：RuleClassifier/LlmClassifier/CompositeClassifier
- **DenialTracker**：连续拒绝自动回退
- **PermissionMode**：Default/Plan/Auto/AcceptEdits/BypassPermissions/DontAsk/Bubble

### Subagent 子代理系统

Sync/Fork/Teammate 三种执行模式。SubagentBuilder/Registry/Executor/Hooks。Team 协作：Coordinator + Mailbox。

---

## 上一版本：v1.0.0

### 记忆工具自动注入

一行代码让 Agent 拥有持久记忆——无需手动接线：

```rust
let store = Arc::new(InMemoryStore::new());
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .with_memory_tools(store)  // 自动注册 remember + recall + search_memory + forget
    .build()?;
```

### 多模态支持

发送图片和文件——兼容 OpenAI Vision 和 Anthropic API：

```rust
let msg = Message::user_with_image(
    "这张图片里有什么？",
    "image/png",
    base64_data,
);
```

### 声明式工作流（YAML/JSON）

无需写 Rust 即可定义 Agent 图：

```yaml
name: research_pipeline
nodes:
  - name: researcher
    type: agent
    model: qwen3-max
    system_prompt: "你是一个研究助手"
    input_key: task
    output_key: research
  - name: writer
    type: agent
    model: qwen3-max
    system_prompt: "你是一个写作助手"
    input_key: research
    output_key: result
edges:
  - from: researcher
    to: writer
entry: researcher
finish: [writer]
```

```rust
let graph = Graph::from_yaml("workflow.yaml")?;
let result = graph.run(state).await?;
```

### 统一重试策略

一套策略覆盖所有外部调用：

```rust
let policy = RetryPolicy::new(3, Duration::from_millis(500))
    .max_delay(Duration::from_secs(30))
    .jitter(true);

let response = with_retry(&policy, || llm_client.chat(request)).await?;
```

### 动态工具管理

对话中途按阶段切换工具集：

```rust
agent.add_tool(Box::new(SearchWebTool));
agent.remove_tool("search_web");
agent.replace_tool(Box::new(SaferExecuteCodeTool));
```

---

## Workspace 结构

```
echo-agent/
├── echo-core/         核心 trait 与类型（Tool、LlmClient、Agent、Guard、Retry 等）
├── echo-macros/       过程宏（#[tool]、#[callback]、#[guard]、#[handler] 等）
├── echo-providers/    LLM 提供方实现（OpenAI、Anthropic、Ollama）
├── echo-mcp/          MCP 协议客户端（stdio、SSE、HTTP 传输）
├── echo-channels/     IM 通道插件（QQ Bot、飞书）
├── src/               主 crate —— Agent 引擎、记忆、技能、工具、工作流等
├── examples/          40 个可运行示例
├── docs/              双语文档（en + zh）
└── skills/            外部技能包（Markdown 格式）

> **注意**：echo-agent 是纯库框架。开箱即用的 Agent 应用（含 CLI 和 Web UI）请参见 [echo-agent-cli](https://github.com/EchoYue-lp/echo-agent-cli)。
```

终端用户只需依赖根 `echo_agent` crate，它会重新导出所有公共 API。

---

## 快速上手

### 前置条件

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 配置模型

在项目根目录创建 `echo-agent.yaml`（或设置 `$ECHO_AGENT_CONFIG`）：

```yaml
models:
  qwen3-max:
    base_url: https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions
    api_key: ${DASHSCOPE_API_KEY}

  gpt-4o:
    provider: openai
    api_key: ${OPENAI_API_KEY}
```

### 运行示例

```bash
cargo run --example demo01_tools
cargo run --example demo25_macros
cargo run --example demo34_workflow_stream
cargo run --example demo36_multimodal

# IM 通道（需配置环境变量）
cargo run --example demo38_im_channels --features channels
```

---

## 核心概念

echo-agent 围绕几个关键概念构建，支持灵活、生产就绪的 Agent 开发：

### 1. ReAct 引擎 —— Thought → Action → Observation 循环
echo-agent 的基础是 ReAct（推理 + 执行）模式，内置 Chain-of-Thought 提示。Agent 逐步思考，决定调用哪个工具，观察结果，直到得出最终答案。

```rust
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("你是一个有帮助的助手")
    .build()?;
let answer = agent.execute("42 * 1337 等于多少？").await?;
```

### 2. 工具系统 —— `#[tool]` 宏 + 自动 JSON Schema
将工具定义为简单的异步函数。`#[tool]` 宏自动生成参数模式、描述和 `TypedTool` 实现。

```rust
use echo_agent::{tool, prelude::*};

#[tool(name = "weather", description = "查询城市天气")]
async fn weather(city: String) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{city} 晴天")))
}

// 使用：agent.add_tool(Box::new(WeatherTool));
```

### 3. 双层记忆 —— Store + Checkpointer
- **Store**：长期键值存储，支持命名空间隔离
- **Checkpointer**：会话历史记录，支持重启恢复

```rust
let store = Arc::new(InMemoryStore::new());
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .with_memory_tools(store)  // 自动注入 remember/recall/search/forget
    .build()?;
```

### 4. 多模态消息 —— 文本、图片、文件同消息发送
发送和接收图片（base64 或 URL）和文件附件，兼容 OpenAI Vision 和 Anthropic API。

```rust
let msg = Message::user_with_image(
    "这张图片里有什么？",
    "image/png",
    base64_data,
);
```

### 5. 上下文压缩 —— 滑动窗口、LLM 摘要、混合模式
通过可配置的压缩策略管理 Token 限制，保留对话上下文。

```rust
agent.set_compressor(Box::new(SlidingWindowCompressor::new(4096)));
```

### 6. 统一重试策略 —— 一套策略覆盖所有外部调用
一次性配置重试、超时和退避，应用于 LLM 调用、MCP 请求、A2A 通信和沙箱执行。

```rust
let policy = RetryPolicy::new(3, Duration::from_millis(500))
    .max_delay(Duration::from_secs(30))
    .jitter(true);
let response = with_retry(&policy, || llm_client.chat(request)).await?;
```

### 7. 动态工具管理 —— 对话过程中增/删/换工具
根据对话阶段或用户需求调整工具集，无需重启 Agent。

```rust
agent.add_tool(Box::new(SearchWebTool));
agent.remove_tool("search_web");
agent.replace_tool(Box::new(SaferExecuteCodeTool));
```

### 8. 人工介入 —— 关键操作审批门
通过控制台、Webhook 或 WebSocket 接口，在执行敏感工具前要求人工审批。

```rust
let approval = ConsoleApproval::new();
agent.set_human_loop_handler(Box::new(approval));
```

### 9. 多 Agent 编排 —— Orchestrator + SubAgent 团队
协调多个专业 Agent，支持上下文隔离和交接协议。

```rust
let orchestrator = Orchestrator::new();
orchestrator.register("math", math_agent);
orchestrator.register("writer", writer_agent);
```

### 10. 技能系统 —— 渐进式能力披露
相关工具和提示的包，可按需发现、激活和使用。

```rust
agent.load_skill("web_research").await?;  // 加载 SKILL.md + 注册工具
```

### 11. MCP 协议 —— 连接任意 Model Context Protocol 服务器
通过标准化的 MCP 服务器集成文件系统、数据库、浏览器等资源。

```rust
let mut mcp = McpManager::new();
let tools = mcp.connect(McpServerConfig::stdio(
    "filesystem", "npx", vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
)).await?;
agent.add_tools(tools);
```

### 12. Plan-and-Execute —— 执行前显式规划阶段
Planner Agent 创建任务 DAG，Executor Agent 逐步执行，支持重新规划。

```rust
let planner = PlanExecuteAgent::new(planner_config, executor_config);
let result = planner.execute("研究量子计算趋势").await?;
```

### 13. 流式输出 —— 实时 Token 级输出
接收 `AgentEvent` 流，包括 Token、工具调用和最终答案的实时事件。

```rust
let mut stream = agent.execute_stream("解释量子纠缠").await?;
while let Some(event) = stream.next().await {
    match event? {
        AgentEvent::Token(t) => print!("{t}"),
        AgentEvent::FinalAnswer(a) => { println!("\n{a}"); break; }
        _ => {}
    }
}
```

### 14. 结构化输出 —— LLM 响应转为类型化 Rust 结构体
使用 JSON Schema 验证从 LLM 响应中提取结构化数据。

```rust
#[derive(Serialize, Deserialize)]
struct Contact { name: String, email: String, phone: String }
let contacts: Vec<Contact> = agent.extract("从这段文本中提取联系人...").await?;
```

### 15. 声明式工作流 —— YAML/JSON 工作流定义
无需编写 Rust 代码即可定义 Agent 图。

```yaml
name: research_pipeline
nodes:
  - name: researcher
    type: agent
    model: qwen3-max
    input_key: task
    output_key: research
edges:
  - from: researcher
    to: writer
```

### 16. 护栏系统 —— 基于规则和 LLM 的内容过滤
通过可定制的护栏管道在输入和输出时阻止或修改不安全内容。

```rust
#[guard(name = "length-limit")]
async fn check_length(content: &str, _: GuardDirection) -> Result<GuardResult> {
    if content.len() > 50000 {
        Ok(GuardResult::Block { reason: "内容过长".into() })
    } else {
        Ok(GuardResult::Pass)
    }
}
```

### 17. 图工作流引擎 —— LangGraph 风格状态机
构建复杂工作流，支持线性管道、条件分支、循环和并行扇出/扇入。

```rust
let graph = GraphBuilder::new("etl_pipeline")
    .add_function_node("extract", |state| Box::pin(async move {
        state.set("data", vec!["hello", "world"]);
        Ok(())
    }))
    .add_edge("extract", "transform")
    .build()?;
```

### 18. IM 通道 —— 将 Agent 部署到消息平台
通过自动 Token 管理和重连机制，将你的 Agent 连接到 QQ（WebSocket）和飞书（Webhook）。

```rust
let mut manager = ChannelManager::new();
manager.register(Box::new(QqChannel::new(qq_config)?));
manager.register(Box::new(FeishuChannel::new(feishu_config)?));
manager.start_all(handler).await?;
```

### 19. 宏系统 —— 常见模式的声明式 API
`#[tool]`、`#[callback]`、`#[guard]`、`#[handler]`、`agent!{}`、`messages![]` 等。

```rust
#[callback]
impl MyCallback {
    async fn on_tool_start(&self, _agent: &str, tool: &str, _args: &serde_json::Value) {
        println!("[工具调用] {tool}");
    }
}
```

---

## 宏速查

| 宏 | 类型 | 用途 |
|----|------|------|
| `#[tool]` | 过程宏 | 从 async fn 生成 TypedTool |
| `#[callback]` | 过程宏 | 从 impl 块生成 AgentCallback |
| `#[guard]` | 过程宏 | 从 async fn 生成 Guard |
| `#[handler]` | 过程宏 | 从 impl 块生成 HumanLoopHandler |
| `#[compressor]` | 过程宏 | 从 async fn 生成 ContextCompressor |
| `#[permission_policy]` | 过程宏 | 从 async fn 生成 PermissionPolicy |
| `#[audit_logger]` | 过程宏 | 从 impl 块生成 AuditLogger |
| `agent!{}` | 声明宏 | 声明式 Agent 构建 |
| `messages![]` | 声明宏 | 快速构建消息列表 |
| `tool_params!{}` | 声明宏 | 快速构建工具参数 Schema |
| `chat_request!{}` | 声明宏 | 快速构建聊天请求 |

---

## 示例

| 示例 | 演示内容 |
|------|---------|
| [`demo01_tools`](examples/demo01_tools.rs) | `#[tool]` 宏定义与调用 |
| [`demo02_tasks`](examples/demo02_tasks.rs) | DAG 任务规划 |
| [`demo03_approval`](examples/demo03_approval.rs) | 人工审批 |
| [`demo04_suagent`](examples/demo04_suagent.rs) | Orchestrator + SubAgent |
| [`demo05_compressor`](examples/demo05_compressor.rs) | 上下文压缩 |
| [`demo06_mcp`](examples/demo06_mcp.rs) | MCP 工具服务器 |
| [`demo07_skills`](examples/demo07_skills.rs) | 内置技能 |
| [`demo08_external_skills`](examples/demo08_external_skills.rs) | 外部技能加载 |
| [`demo09_file_shell`](examples/demo09_file_shell.rs) | 文件和 Shell 工具 |
| [`demo10_streaming`](examples/demo10_streaming.rs) | 流式输出 |
| [`demo11_callbacks`](examples/demo11_callbacks.rs) | 生命周期回调 |
| [`demo12_resilience`](examples/demo12_resilience.rs) | 重试、超时、容错 |
| [`demo13_tool_execution`](examples/demo13_tool_execution.rs) | 工具执行配置 |
| [`demo14_memory_isolation`](examples/demo14_memory_isolation.rs) | 记忆隔离 |
| [`demo15_structured_output`](examples/demo15_structured_output.rs) | JSON Schema 结构化输出 |
| [`demo16_testing`](examples/demo16_testing.rs) | Mock 测试 |
| [`demo17_chat`](examples/demo17_chat.rs) | 交互式对话 |
| [`demo18_semantic_memory`](examples/demo18_semantic_memory.rs) | 语义记忆 |
| [`demo19_guard`](examples/demo19_guard.rs) | 护栏系统 |
| [`demo20_audit`](examples/demo20_audit.rs) | 审计日志 |
| [`demo21_handoff`](examples/demo21_handoff.rs) | Agent 间 Handoff |
| [`demo22_plan_execute`](examples/demo22_plan_execute.rs) | Plan-and-Execute |
| [`demo23_a2a`](examples/demo23_a2a.rs) | A2A 协议 |
| [`demo24_topology`](examples/demo24_topology.rs) | 拓扑可视化 |
| [`demo25_macros`](examples/demo25_macros.rs) | 宏系统综合展示 |
| [`demo26_provider_factory`](examples/demo26_provider_factory.rs) | 动态 LLM 提供方工厂 |
| [`demo27_sqlite_memory`](examples/demo27_sqlite_memory.rs) | SQLite 持久化（FTS5 + 向量搜索） |
| [`demo28_workflow`](examples/demo28_workflow.rs) | Workflow 管道抽象 |
| [`demo29_sandbox`](examples/demo29_sandbox.rs) | 沙箱代码执行 |
| [`demo30_mcp_server`](examples/demo30_mcp_server.rs) | MCP 服务端模式 |
| [`demo31_memory_tools`](examples/demo31_memory_tools.rs) | **记忆工具自动注入** |
| [`demo32_token_budget`](examples/demo32_token_budget.rs) | **Token 预算管控** |
| [`demo33_retry_policy`](examples/demo33_retry_policy.rs) | **统一重试策略** |
| [`demo34_workflow_stream`](examples/demo34_workflow_stream.rs) | **工作流流式事件** |
| [`demo35_dynamic_tools`](examples/demo35_dynamic_tools.rs) | **动态工具注册/注销** |
| [`demo36_multimodal`](examples/demo36_multimodal.rs) | **多模态消息** |
| [`demo37_declarative_workflow`](examples/demo37_declarative_workflow.rs) | **YAML/JSON 声明式工作流** |
| [`demo38_im_channels`](examples/demo38_im_channels.rs) | **IM 平台接入（QQ + 飞书）** |
| [`demo39_workflow`](examples/demo39_workflow.rs) | **图工作流引擎与 SharedState** |
| [`demo40_snapshot`](examples/demo40_snapshot.rs) | **Agent 状态快照与回滚** |

---

## 兼容性

支持任意 **OpenAI 兼容** API，以及原生 Anthropic、Ollama 支持：

| Provider | 接入地址 |
|----------|---------|
| OpenAI | `https://api.openai.com/v1` |
| Anthropic | `https://api.anthropic.com/v1`（原生） |
| DeepSeek | `https://api.deepseek.com/v1` |
| 阿里云 Qwen | `https://dashscope.aliyuncs.com/compatible-mode/v1` |
| Ollama（本地） | `http://localhost:11434`（原生） |
| LM Studio | `http://localhost:1234/v1` |

---

## 文档

完整文档位于 [`docs/`](./docs/)：

**中文**（[`docs/zh/`](./docs/zh/README.md)）

- [ReAct Agent](docs/zh/01-react-agent.md)
- [工具系统](docs/zh/02-tools.md)
- [记忆系统](docs/zh/03-memory.md)
- [上下文压缩](docs/zh/04-compression.md)
- [人工介入](docs/zh/05-human-loop.md)
- [多 Agent 编排](docs/zh/06-subagent.md)
- [Skill 系统](docs/zh/07-skills.md)
- [MCP 协议](docs/zh/08-mcp.md)
- [DAG 任务](docs/zh/09-tasks.md)
- [流式输出](docs/zh/10-streaming.md)
- [结构化输出](docs/zh/11-structured-output.md)
- [Mock 测试](docs/zh/12-mock.md)
- [多轮对话](docs/zh/13-chat.md)
- [语义搜索](docs/zh/14-semantic-search.md)
- [IM 通道](docs/zh/15-im-channels.md)
- [自我反思 Agent](docs/zh/15-self-reflection.md)
- [Plan-and-Execute](docs/zh/16-plan-execute.md)
- [图工作流](docs/zh/17-graph-workflow.md)
- [护栏系统](docs/zh/18-guard-system.md)

**English**（[`docs/en/`](./docs/en/README.md)）

---

## 参与贡献

```bash
git clone https://github.com/EchoYue-lp/echo-agent
cd echo-agent
cargo build
cargo test --lib
cargo run --example demo01_tools
```

提交 PR 前：
- 运行 `cargo fmt` 和 `cargo clippy`
- 为新功能添加测试
- 更新 `docs/` 下相关文档

---

## License

MIT © echo-agent contributors
