<div align="center">

# echo-agent

### 面向 Rust 的生产级 AI Agent 框架

**ReAct 引擎 • 多 Agent • 记忆 • 流式 • MCP • IM 通道 • 工作流**

[![crates.io](https://img.shields.io/crates/v/echo_agent?color=brightgreen)](https://crates.io/crates/echo_agent)
[![docs.rs](https://docs.rs/echo_agent/badge.svg)](https://docs.rs/echo_agent)
[![CI](https://github.com/EchoYue-lp/echo-agent/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/EchoYue-lp/echo-agent/actions)
[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![OpenAI Compatible](https://img.shields.io/badge/API-OpenAI%20%E5%85%BC%E5%AE%B9-green)](https://platform.openai.com/docs/api-reference)
[![Async](https://img.shields.io/badge/runtime-tokio-blue)](https://tokio.rs/)

[English](./README.md) &middot; [文档中心](./docs/zh/README.md) &middot; [Rust 学习指南](./docs/zh/rust-learning/README.md) &middot; [示例](./examples/) &middot; [更新日志](./CHANGELOG.md)

</div>

---

## 快速上手

添加到 `Cargo.toml`：

```toml
[dependencies]
echo-agent = "0.2.0"
tokio = { version = "1", features = ["full"] }
```

定义工具、运行 Agent——不到 20 行代码：

```rust,no_run
use echo_agent::prelude::*;
use echo_agent::{agent, tool};

#[tool(name = "add", description = "两数相加")]
async fn add(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a + b)))
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut agent = agent! {
        model: "qwen3.7-max",
        system_prompt: "你是一个计算助手",
        tools: [AddTool],
    }?;

    let answer = agent.execute("计算 1337 × 42").await?;
    println!("{answer}");
    Ok(())
}
```

---

## 为什么选择 echo-agent？

绝大多数 AI Agent 框架基于 Python。**echo-agent** 将完整的现代 Agent 框架带入 Rust——与 [LangGraph](https://github.com/langchain-ai/langgraph)、[CrewAI](https://github.com/crewAIInc/crewAI)、[AutoGen](https://github.com/microsoft/autogen) 功能对齐，同时提供只有 Rust 才能带来的**性能、类型安全和可靠性**。

| | echo-agent | LangGraph | CrewAI | AutoGen |
|---|---|---|---|---|
| **语言** | Rust | Python | Python | Python |
| **内存安全** | 编译时保证 | GC | GC | GC |
| **ReAct 循环** | 内置 | 内置 | 内置 | 内置 |
| **工具系统** | `#[tool]` 宏 + JSON Schema | 装饰器 | 装饰器 | 函数调用 |
| **多 Agent** | Subagent 调度 + 团队 | 图结构 | Crew 模式 | 对话模式 |
| **流式输出** | 原生 async stream | 回调 | 有限 | 回调 |
| **MCP 协议** | 原生（stdio/SSE/HTTP） | 通过 LangChain | 无 | 无 |
| **IM 通道** | QQ + 飞书内置 | 无 | 无 | 无 |
| **工作流** | Graph + DAG + Sequential | StateGraph | Sequential | Sequential |
| **上下文压缩** | SlidingWindow + LLM + Hybrid | 无 | 无 | 无 |
| **护栏系统** | 规则 + LLM 过滤 | 无 | 无 | 无 |
| **沙箱** | Local + Docker + K8s | 无 | 无 | Docker |
| **单二进制部署** | 是 | 否 | 否 | 否 |

### 5 行代码接入 IM

```rust
let mut manager = ChannelManager::new();
manager.register(Box::new(QqChannel::new(qq_config)?));
manager.register(Box::new(FeishuChannel::new(feishu_config)?));
manager.start_all(handler).await?;
```

### 运行示例

```bash
cargo run --example demo01_tools          # 自定义工具
cargo run --example demo25_macros         # 宏系统
cargo run --example demo34_workflow_stream # 工作流流式
cargo run --example demo36_multimodal     # 多模态消息
cargo run --example demo38_im_channels --features channels  # IM 通道
```

---

## 架构

```
                              ┌─────────────┐
                              │   你的应用    │
                              └──────┬───────┘
                                     │
                    ┌────────────────▼────────────────┐
                    │          ReactAgent              │
                    │                                  │
                    │  ┌──────────┐  ┌──────────────┐  │
                    │  │  上下文    │  │    工具       │  │
                    │  │  管理器    │  │   管理器      │  │
                    │  │(压缩)     │  │(重试/限流)    │  │
                    │  └──────────┘  └──────────────┘  │
                    │                                  │
                    │  ┌──────────┐  ┌──────────────┐  │
                    │  │  记忆     │  │   人工        │  │
                    │  │Store+Cp  │  │   审批        │  │
                    │  └──────────┘  └──────────────┘  │
                    │                                  │
                    │  ┌──────────┐  ┌──────────────┐  │
                    │  │  技能     │  │   子代理      │  │
                    │  │  注册表   │  │   注册表      │  │
                    │  └──────────┘  └──────────────┘  │
                    └────────────────┬────────────────┘
                                     │
              ┌──────────────────────▼──────────────────────┐
              │              LLM 提供方                       │
              │  OpenAI · Anthropic · DeepSeek · Qwen · Ollama │
              └─────────────────────────────────────────────┘
```

---

## 功能矩阵

echo-agent 提供跨越 8 个 crate 的 **67 个注册工具**。Prelude 只导出通用 trait 与 builder；具体工具使用 `echo_agent::tools::files::*` 等稳定路径，启用相应 feature 后可通过 `echo_agent::tools::register_all_tools` 批量注册。

### 核心

| 能力 | 描述 | API 预览 |
|------|------|---------|
| **ReAct 引擎** | Thought → Action → Observation 循环 | `agent.execute("task").await?` |
| **工具系统** | `#[tool]` 宏 + 自动 JSON Schema + 超时重试 | `#[tool(name = "calc")] async fn calc(...)` |
| **记忆系统** | `Store`（长期 KV）+ `RuntimeStateStore`（崩溃恢复）+ `ConversationStore`（会话历史） | `.with_memory_tools(store)` |
| **上下文压缩** | 滑动窗口 / LLM 摘要 / 混合管道 | `SlidingWindowCompressor::new(4096)` |
| **Token 预算** | 自动截断 + think 前触发压缩 | `.max_tool_output_tokens(2000)` |
| **统一重试** | 一套策略覆盖 LLM、MCP、A2A、沙箱 | `with_retry(&policy, \|\| ...)` |
| **动态工具** | 对话中增 / 删 / 换工具 | `agent.remove_tool("old")` |
| **流式输出** | 实时 `AgentEvent` 流（Token + 工具调用） | `agent.execute_stream(task).await?` |
| **结构化输出** | LLM 输出 → Rust 类型（JSON Schema） | `agent.extract::<Contact>(text)` |
| **多模态** | 文本 + 图片 + 文件混合消息 | `Message::user_with_image(...)` |
| **护栏系统** | 规则 / LLM 内容过滤 | `#[guard(name = "safety")] async fn ...` |
| **权限模型** | 声明式工具权限 + 统一权限服务 | `PermissionService::from_provider(...)` |
| **审计日志** | 结构化事件 + 可插拔后端 | `agent.set_audit_logger(...)` |
| **宏系统** | 11 个宏：`#[tool]`、`agent!{}`、`messages![]`... | `agent! { model: "..", tools: [...] }` |

### 多 Agent 与编排

| 能力 | 描述 | API 预览 |
|------|------|---------|
| **Subagent** | Sync / Fork / Teammate 三种模式 | `agent.register_agent(sub)` |
| **任务图** | 单一依赖图上的版本化任务 CRUD | `task_create` / `task_update` / `task_list` |
| **自我审查** | LLM 质量评估作为工具 | `ReviewTool::new(critic)` |
| **图工作流** | 线性、条件、循环、并行扇出/扇入 | `GraphBuilder::new("pipeline")` |
| **DAG 任务** | 版本化 CRUD 与依赖感知执行 | `TaskRevisionService::new(...)` |
| **声明式工作流** | YAML/JSON 定义图——无需 Rust 代码 | `Graph::from_yaml("wf.yaml")?` |

### 集成

| 能力 | 描述 | API 预览 |
|------|------|---------|
| **MCP 协议** | 接入任意 MCP 服务器（stdio/SSE/HTTP） | `mcp.connect(McpServerConfig::stdio(...))` |
| **A2A 协议** | Agent Card 发布、跨框架协作 | `A2AServer::bind("0.0.0.0:3000")` |
| **Skill 系统** | 渐进式披露：发现 → 激活 → 使用 | `agent.load_skill("web_research")` |
| **IM 通道** | QQ Bot（WebSocket）+ 飞书（Webhook） | `ChannelManager::new()` |
| **Web 工具** | 搜索 + 网页获取 | `WebSearchTool::auto()` |
| **论文检索工具** | ArXiv、Semantic Scholar、PDF 下载、BibTeX 生成 | `ArxivSearchTool` |
| **媒体工具** | PDF、Excel、Word、图片分析 | `ImageAnalysisTool` |
| **数据工具** | Polars 驱动的过滤、聚合、统计 | `DataReadTool` |
| **沙箱** | Local / Docker / K8s 代码执行 | `LocalSandbox::new()` |
| **OpenTelemetry** | 分布式追踪与指标 | `init_telemetry(&config)` |
| **快照/回滚** | 任意时刻捕获和恢复 Agent 状态 | `agent.snapshot()` / `agent.rollback(1)` |
| **熔断器** | LLM 不可用时自动快速失败 | `agent.set_circuit_breaker(config)` |

---

## Feature Flags

```toml
# 最小化 —— 仅 ReAct 引擎（默认）
echo-agent = "0.2.0"

# 显式禁用默认 feature（等价于默认安装）
echo-agent = { version = "0.2.0", default-features = false }

# 完整功能
echo-agent = { version = "0.2.0", features = ["full"] }

# 按需选择
echo-agent = { version = "0.2.0", default-features = false, features = ["mcp", "web"] }
```

| Feature | 启用 | 关键依赖 |
|---------|------|---------|
| `full` | 启用下列全部 feature | — |
| `a2a` | Agent-to-Agent 协议 | `axum`, `jsonwebtoken` |
| `mcp` | MCP 协议客户端 | — |
| `lsp` | Language Server Protocol 集成 | — |
| `sqlite` | SQLite 持久化状态 | `rusqlite` |
| `telemetry` | OpenTelemetry 追踪与指标 | `opentelemetry` |
| `human-loop` | 人工审批 | `tokio-tungstenite` |
| `topology` | Agent 拓扑 | — |
| `tasks` | DAG 任务管理 | — |
| `subagent` | Subagent 编排 | — |
| `web` | Web 搜索 + 获取工具 | `scraper`, `html2text` |
| `media` | PDF、Excel、Word、图片工具 | `lopdf`, `calamine`, `docx-rs` |
| `data` | Polars 数据分析 | `polars` |
| `statistics` | 统计分析工具 | — |
| `channels` | QQ Bot + 飞书集成 | `echo-channels` |
| `git` | Git 操作工具 | — |
| `database` | 数据库查询工具 | `sqlx` |
| `rag` | RAG 检索工具 | — |
| `research` | ArXiv、Semantic Scholar、PDF 下载、BibTeX 工具 | `quick-xml` |
| `chart` | 图表生成工具 | — |
| `content-guard` | 内容安全护栏 | — |
| `project-rules` | 项目规则加载 | — |
| `shell` | Shell 命令执行工具 | — |
| `files` | 文件读写工具 | — |
| `eval` | 评估原语 | — |
| `improve` | 自改进原语 | — |
| `testing` | Mock 与测试辅助 | — |

---

## Workspace 结构

```
echo-agent/
├── echo-core/           核心 trait：Tool、Agent、LlmClient、Guard、Error、Retry
├── echo-macros/         过程宏：#[tool]、#[callback]、#[guard]、#[handler]
├── echo-execution/      沙箱、技能和工具执行
├── echo-state/          记忆、压缩和审计日志
├── echo-orchestration/  工作流、人工审批和 DAG 任务
├── echo-integration/    LLM 提供方、MCP 和 IM 通道（QQ/飞书）
├── echo-tools/          领域工具：chart、data、database、git、media、web、rag
├── echo-rust-learning/  非发布的 Rust 教学 crate 与离线练习
├── src/                 Agent 引擎、重导出和门面层
├── examples/            可运行的框架功能示例
├── docs/                双语文档（en + zh）
└── echo-agent.example.yaml  示例配置
```

> **注意：** `echo-agent` 是纯库框架。开箱即用的应用（含 CLI、Web UI、WebSocket）请参见 [echo-agent-cli](https://github.com/EchoYue-lp/echo-agent-cli)。

---

## 配置

在项目根目录创建应用配置 `echo-agent.yaml`：

```yaml
model:
  default_model_id: openai:gpt-5.6-sol
  provider: openai
  name: gpt-5.6-sol
  max_tokens: 4096
  temperature: 0.7

model_providers:
  openai:
    name: OpenAI
    base_url: https://api.openai.com/v1
    api_key_env: OPENAI_API_KEY
    default_api_protocol: responses
    requires_api_key: true

configured_models:
  - id: openai:gpt-5.6-sol
    display_name: GPT-5.6 Sol
    provider: openai
    model: gpt-5.6-sol
    api_protocol: responses
    input_modalities: [text, image]
    enabled: true

agent:
  name: my-assistant
  system_prompt: "你是一个有帮助的助手。"
  max_iterations: 10
  enable_tools: true
  enable_memory: true

channels:
  qq:
    enabled: false
    app_id: ${QQ_APP_ID}
    client_secret: ${QQ_CLIENT_SECRET}
  feishu:
    enabled: false
    app_id: ${FEISHU_APP_ID}
    app_secret: ${FEISHU_APP_SECRET}
    mode: long_poll
  session:
    timeout_minutes: 60
    reset_keywords: ["重置对话", "新对话", "清除记忆"]
    reset_commands: ["/reset", "/clear", "/new"]

mcp:
  config_path: ./mcp.json

server:
  host: 0.0.0.0
  port: 3000

logging:
  level: info
```

说明：

- `model_providers:` 保存连接和认证；`configured_models:` 保存每个模型明确选择的协议以及文本/图片/音频/视频输入能力。
- 思考控制由框架根据 Provider endpoint、API 协议和模型名集中解析，不需要用户配置厂商原始字段。

通过环境变量设置密钥：

```bash
export DASHSCOPE_API_KEY=sk-xxx      # 阿里云 Qwen
export DEEPSEEK_API_KEY=sk-xxx       # DeepSeek
export OPENAI_API_KEY=sk-xxx         # OpenAI
export ANTHROPIC_API_KEY=sk-ant-xxx  # Anthropic
export QQ_APP_ID=your-qq-app-id
export QQ_CLIENT_SECRET=your-qq-client-secret
export FEISHU_APP_ID=your-feishu-app-id
export FEISHU_APP_SECRET=your-feishu-app-secret
```

---

## 亮点

- **67 个注册工具** — ReAct 循环、数据分析、论文检索、Web、媒体、RAG、数据库等
- **可运行示例与教学 crate** — 框架验收示例和 Rust 基础课程分别维护
- **全模块单元测试** — 覆盖核心路径的测试
- **8 个生产 crate + 1 个教学 crate** — 生产依赖保持单向，教学代码不进入框架运行时
- **多模态** — 文本、图片（base64 & URL）、文件附件混合消息
- **IM 集成** — QQ Bot（WebSocket）& 飞书（Webhook）开箱即用
- **声明式工作流** — 用 YAML/JSON 定义 Agent 图，无需写 Rust 代码
- **统一重试** — 一套 `RetryPolicy` 覆盖所有外部调用（LLM、MCP、A2A、沙箱）
- **零成本抽象** — 编译为原生代码，无运行时开销

---

## 核心概念

echo-agent 围绕多个关键概念构建，支持灵活、生产级的 Agent 开发：

### 1. ReAct 引擎 — Thought → Action → Observation 循环

echo-agent 的基础是 ReAct（Reasoning + Acting）模式，内置 Chain-of-Thought 提示。Agent 逐步思考、决定调用哪个工具、观察结果，直到得出最终答案。

```rust
let agent = ReactAgentBuilder::new()
    .model("qwen3.7-max")
    .system_prompt("你是一个有帮助的助手")
    .build()?;
let answer = agent.execute("42 * 1337 等于多少？").await?;
```

三种 Builder 预设：

```rust
// 最小化 — 无工具、无记忆，纯对话
let agent = ReactAgentBuilder::simple("qwen3.7-max", "帮助用户")?;

// 标准版 — 工具 + CoT 启用
let agent = ReactAgentBuilder::standard("qwen3.7-max", "assistant", "帮助用户")?;

// 全功能 — 工具 + 记忆 + 任务 + CoT
let agent = ReactAgentBuilder::full_featured("qwen3.7-max", "assistant", "帮助用户")?;
```

### 2. 工具系统 — `#[tool]` 宏 + 自动 JSON Schema

将工具定义为简单的异步函数。`#[tool]` 宏自动生成参数 Schema、描述和 `TypedTool` 实现。

```rust
use echo_agent::{tool, prelude::*};

#[tool(name = "weather", description = "查询城市天气")]
async fn weather(city: String) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{city} 晴天")))
}

// 使用：agent.add_tool(Box::new(WeatherTool));
```

内置媒体工具（feature `media`）：PDF 提取/信息、Excel 读取/信息/导出CSV、Word 读取/信息/结构、图片分析、文本读取/搜索/统计/处理/导出。

内置数据工具（feature `data`）：基于 Polars 的读取/过滤/聚合/统计/转换/导出。

### 3. 记忆系统 — Store + RuntimeStateStore + ConversationStore

- **Store**：长期键值存储，支持命名空间隔离（`InMemoryStore`、`FileStore`、`SqliteStore`）
- **RuntimeStateStore**：完整运行时检查点（消息 + 计划 + 激活技能 + 阻塞原因），用于跨进程崩溃恢复（`SqliteRuntimeStateStore`）
- **ConversationStore**：用户可见的对话历史投影，run 收尾时自动持久化

一行代码让 Agent 拥有持久记忆——无需手动工具接线：

```rust
let store = Arc::new(InMemoryStore::new());
let agent = ReactAgentBuilder::new()
    .model("qwen3.7-max")
    .with_memory_tools(store)  // 注册 remember + recall + search_memory + forget
    .build()?;
```

### 4. 多模态消息 — 文本、图片、文件合一

在同一条消息中发送/接收图片（base64 或 URL）和文件附件，兼容 OpenAI Vision 和 Anthropic API。

```rust
let msg = Message::user_with_image(
    "这张图片里有什么？",
    "image/png",
    base64_data,
);
```

### 5. 上下文压缩 — 滑动窗口、LLM 摘要、混合

通过可配置的压缩策略管理 Token 限制，保留对话上下文。

```rust
agent.set_compressor(Box::new(SlidingWindowCompressor::new(4096)));
```

三种策略：
- **SlidingWindow** — 在 Token 预算内保留最近的消息
- **SummaryCompressor** — 使用 LLM 摘要旧消息
- **HybridCompressor** — 组合两者以获得最佳质量

### 6. 统一重试策略 — 一套策略覆盖所有外部调用

配置一次重试、超时和退避，应用于 LLM 调用、MCP 请求、A2A 通信和沙箱执行。

```rust
let policy = RetryPolicy::new(3, Duration::from_millis(500))
    .max_delay(Duration::from_secs(30))
    .jitter(true);
let response = with_retry(&policy, || llm_client.chat(request)).await?;
```

### 7. 动态工具管理 — 对话中增/删/换工具

无需重启 Agent 即可根据对话阶段或用户需求调整工具集。

```rust
agent.add_tool(Box::new(SearchWebTool));
agent.remove_tool("search_web");
agent.replace_tool(Box::new(SaferExecuteCodeTool));
```

### 8. 人工介入 — 关键操作的审批门控

通过控制台、Webhook 或 WebSocket 接口要求人工审批后执行敏感工具。

```rust
let approval = ConsoleApproval::new();
agent.set_human_loop_handler(Box::new(approval));
```

完整的 7 阶段权限流水线（灵感来自 Claude Code）：

```
Bypass → Plan → Rules(deny-first) → ProtectedPaths → Cache(TTL) → DenialTracker → Mode dispatch
```

- **SessionApprovalCache**：可配置 TTL（默认 30 分钟）
- **审计追踪**：`PermissionAuditSink` trait + InMemory/Logging/Composite 实现
- **ProtectedPathChecker**：`.git`/`.env`/`.ssh` 始终受保护
- **AI 分类器**：RuleClassifier/LlmClassifier/CompositeClassifier 用于 Auto 模式
- **DenialTracker**：连续拒绝后自动降级
- **PermissionMode**：Default/Plan/Auto/AcceptEdits/BypassPermissions/DontAsk/Bubble

### 9. 多 Agent 编排 — Orchestrator + Subagent 团队

协调多个专业 Agent，支持上下文隔离和切换协议。

三种执行模式：
- **Sync** — 父 Agent 阻塞等待子 Agent 返回
- **Fork** — 子 Agent 在后台运行，父 Agent 继续执行
- **Teammate** — 独立后台 Subagent，通过 handle 等待或取消

```rust
let orchestrator = Orchestrator::new();
orchestrator.register("math", math_agent);
orchestrator.register("writer", writer_agent);
```

### 10. Skill 系统 — 渐进式能力披露

相关工具和提示的打包，可按需发现、激活和使用。

```rust
agent.load_skill("web_research").await?;  // 加载 SKILL.md + 注册工具
```

预置技能：`code_review`、`data_analyst`、`project-stats`、`python-linter`、`web_researcher`。

### 11. MCP 协议 — 连接任意 Model Context Protocol 服务器

通过标准化 MCP 服务器集成文件系统、数据库、浏览器等资源。

```rust
let mut mcp = McpManager::new();
let tools = mcp.connect(McpServerConfig::stdio(
    "filesystem", "npx", vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
)).await?;
agent.add_tools(tools);
```

支持三种传输方式：**stdio**、**SSE**、**HTTP**。

### 12. 任务图 — 版本化 TaskRun graph

ReactAgent 提供 `task_create`、`task_update`、`task_list`，共同操作一个版本化任务图。Plan 是可编辑、可审阅的 artifact；产品特定的执行策略由应用层适配器提供。

> **注意**: 需要启用 `tasks` feature。

```rust,ignore
let agent = ReactAgentBuilder::new()
    .model("qwen3.7-max")
    .enable_tools()
    .build()?;

let result = agent.execute("为量子计算趋势研究创建任务图").await?;
```

### 13. 流式输出 — 实时逐 Token 输出

实时接收 `AgentEvent` 流，包括 Token、工具调用和最终答案。

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

### 14. 结构化输出 — LLM 响应转 Rust 类型

使用 JSON Schema 验证从 LLM 响应中提取结构化数据。

```rust
#[derive(Serialize, Deserialize)]
struct Contact { name: String, email: String, phone: String }
let contacts: Vec<Contact> = agent.extract("从这段文本中提取联系人信息...").await?;
```

### 15. 声明式工作流 — YAML/JSON 工作流定义

无需写 Rust 代码即可定义 Agent 图。

```yaml
name: research_pipeline
nodes:
  - name: researcher
    type: agent
    model: qwen3.7-max
    system_prompt: "你是研究助手"
    input_key: task
    output_key: research
  - name: writer
    type: agent
    model: qwen3.7-max
    system_prompt: "你是写作助手"
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

### 16. 护栏系统 — 规则和 LLM 驱动的内容过滤

通过可定制的护栏管道在输入和输出端阻止或修改不安全内容。

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

### 17. 图工作流引擎 — LangGraph 风格状态机

构建支持线性管道、条件分支、循环和并行扇出/扇入的复杂工作流。

```rust
let graph = GraphBuilder::new("etl_pipeline")
    .add_function_node("extract", |state| Box::pin(async move {
        state.set("data", vec!["hello", "world"])?;
        Ok(())
    }))
    .add_function_node("transform", |state| Box::pin(async move {
        // 转换数据...
        Ok(())
    }))
    .add_edge("extract", "transform")
    .add_edge("transform", Graph::END)
    .build()?;

let result = graph.run(state).await?;
```

还支持**流式执行**：`graph.run_stream(state).await?` 每个节点产生 `WorkflowEvent`。

### 18. IM 通道 — 将 Agent 部署到即时通讯平台

通过 QQ（WebSocket）和飞书（Webhook）连接你的 Agent，自动管理 Token 和重连。

```rust
// QQ Bot — WebSocket 网关
let qq = QqChannel::new(QqConfig {
    app_id, client_secret,
})?;

// 飞书 — HTTP Webhook
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

功能特性：
- **统一 `ChannelPlugin` 接口** — 实现一个 trait 即可添加新平台
- **自动 Token 管理** — OAuth 缓存和刷新，无需手动处理
- **WebSocket 重连** — 指数退避，永不静默断开
- **消息队列** — 异步 `mpsc` 通道防止高负载下消息丢失
- **白名单支持** — `ChatConfig::with_allow_from()` 实现访问控制

### 19. 宏系统 — 常用模式的声明式 API

`#[tool]`、`#[callback]`、`#[guard]`、`#[handler]`、`agent!{}`、`messages![]` 等。

```rust
#[callback]
impl MyCallback {
    async fn on_tool_start(&self, _agent: &str, tool: &str, _args: &serde_json::Value) {
        println!("[工具] {tool}");
    }
}
```

### 20. Web 工具 — 搜索互联网和获取网页

让你的 Agent 拥有实时互联网访问能力。

```rust
use echo_agent::tools::web::{WebSearchTool, WebFetchTool};

// 自动选择最佳提供商：Tavily > Brave > DuckDuckGo
agent.add_tool(Box::new(WebSearchTool::auto()));
agent.add_tool(Box::new(WebFetchTool::new()));
```

| 提供商 | 费用 | 质量 | 备注 |
|--------|------|------|------|
| DuckDuckGo | 免费 | 中等 | HTML 抓取，无需 API Key |
| Brave | 免费 2k/月 | 高 | 官方 API |
| Tavily | 付费（有免费额度） | 最高 | 为 Agent AI 优化 |

### 21. 自我审查 — 作为工具的质量评估

使用 ReviewTool 让 Agent 评估和优化自己的输出。这遵循业界最佳实践（Hermes、Claude Code），反思是工具能力而非独立的 Agent 类型。

```rust,ignore
let llm_config = LlmConfig::openai(
    std::env::var("OPENAI_API_KEY").unwrap_or_default(),
    "gpt-5.5",
);
let llm_client: Arc<dyn LlmClient> = Arc::from(llm_config.build_client()?);
let critic = Arc::new(LlmCritic::new(llm_client.clone()));
let review_tool = ReviewTool::new(critic);

let agent = ReactAgentBuilder::new()
    .model("gpt-5.5")
    .llm_config(llm_config)
    .llm_client(llm_client)
    .system_prompt("你是技术写手。使用 review 工具自我审查你的作品。")
    .enable_tools()
    .tool(Box::new(review_tool))
    .build()?;

// Agent 现在可以调用 review(task, output) 来评估自己的作品
let result = agent.execute("撰写量子计算摘要，然后审查它").await?;
```

### 22. 快照与回滚 — 时光机调试

```rust
let snapshot_id = agent.snapshot()?;  // 捕获当前状态
// ... 一些出问题的操作 ...
agent.rollback(1)?;                   // 回退 1 步
agent.rollback_to(&snapshot_id)?;     // 或回退到指定快照
```

### 23. 熔断器 — LLM 不可用时自动快速失败

```rust
let cb_config = CircuitBreakerConfig::new()
    .failure_threshold(5)
    .timeout(Duration::from_secs(30));
agent.set_circuit_breaker(cb_config);
```

---

## 宏速查

| 宏 | 类型 | 生成 |
|----|------|------|
| `#[tool]` | 过程宏 | 从 async fn 生成 TypedTool |
| `#[callback]` | 过程宏 | 从 impl 块生成 AgentCallback |
| `#[guard]` | 过程宏 | 从 async fn 生成 Guard |
| `#[handler]` | 过程宏 | 从 impl 块生成 HumanLoopHandler |
| `#[compressor]` | 过程宏 | 从 async fn 生成 ContextCompressor |
| `#[permission_policy]` | 过程宏 | 从 async fn 生成 PermissionPolicy |
| `#[audit_logger]` | 过程宏 | 从 impl 块生成 AuditLogger |
| `agent!{}` | 声明宏 | Agent 构建 |
| `messages![]` | 声明宏 | 消息列表构建 |
| `tool_params!{}` | 声明宏 | JSON Schema 构建 |
| `chat_request!{}` | 声明宏 | ChatRequest 构建 |

---

## 示例

所有示例现在按 `验收样例`、`条件验收样例`、`教学示例` 三类维护。
完整分层清单和维护规则见 `examples/README.md`。

| # | 示例 | 演示内容 |
|---|------|---------|
| 01 | [`demo01_tools`](examples/demo01_tools.rs) | `#[tool]` 宏自定义工具 |
| 02 | [`demo02_tasks`](examples/demo02_tasks.rs) | DAG 任务规划 |
| 03 | [`demo03_approval`](examples/demo03_approval.rs) | 人工审批 |
| 04 | [`demo04_subagent`](examples/demo04_subagent.rs) | 多 Agent 编排 |
| 05 | [`demo05_compressor`](examples/demo05_compressor.rs) | 上下文压缩 |
| 06 | [`demo06_mcp`](examples/demo06_mcp.rs) | MCP 工具服务器 |
| 07 | [`demo07_skills`](examples/demo07_skills.rs) | 内置技能 |
| 08 | [`demo08_external_skills`](examples/demo08_external_skills.rs) | 外部技能加载 |
| 09 | [`demo09_file_shell`](examples/demo09_file_shell.rs) | 文件 & Shell 工具 |
| 10 | [`demo10_streaming`](examples/demo10_streaming.rs) | 流式输出 |
| 11 | [`demo11_callbacks`](examples/demo11_callbacks.rs) | 生命周期回调 |
| 12 | [`demo12_resilience`](examples/demo12_resilience.rs) | 重试与容错 |
| 13 | [`demo13_tool_execution`](examples/demo13_tool_execution.rs) | 工具执行配置 |
| 15 | [`demo15_structured_output`](examples/demo15_structured_output.rs) | JSON Schema 输出 |
| 17 | [`demo17_chat`](examples/demo17_chat.rs) | 交互式对话 |
| 18 | [`demo18_semantic_memory`](examples/demo18_semantic_memory.rs) | 语义记忆 |
| 19 | [`demo19_guard`](examples/demo19_guard.rs) | 护栏系统 |
| 20 | [`demo20_audit`](examples/demo20_audit.rs) | 审计日志 |
| 23 | [`demo23_a2a`](examples/demo23_a2a.rs) | A2A 协议 |
| 24 | [`demo24_topology`](examples/demo24_topology.rs) | 拓扑可视化 |
| 25 | [`demo25_macros`](examples/demo25_macros.rs) | 宏系统展示 |
| 26 | [`demo26_provider_factory`](examples/demo26_provider_factory.rs) | 动态 LLM 工厂 |
| 27 | [`demo27_sqlite_memory`](examples/demo27_sqlite_memory.rs) | SQLite 持久化 |
| 28 | [`demo28_workflow`](examples/demo28_workflow.rs) | 工作流管道 |
| 29 | [`demo29_sandbox`](examples/demo29_sandbox.rs) | 沙箱执行 |
| 30 | [`demo30_mcp_server`](examples/demo30_mcp_server.rs) | MCP 服务端 |
| 31 | [`demo31_memory_tools`](examples/demo31_memory_tools.rs) | 记忆工具注入 |
| 32 | [`demo32_token_budget`](examples/demo32_token_budget.rs) | Token 预算管控 |
| 33 | [`demo33_retry_policy`](examples/demo33_retry_policy.rs) | 统一重试 |
| 34 | [`demo34_workflow_stream`](examples/demo34_workflow_stream.rs) | 工作流流式 |
| 35 | [`demo35_dynamic_tools`](examples/demo35_dynamic_tools.rs) | 动态工具管理 |
| 36 | [`demo36_multimodal`](examples/demo36_multimodal.rs) | 多模态消息 |
| 37 | [`demo37_declarative_workflow`](examples/demo37_declarative_workflow.rs) | YAML/JSON 工作流 |
| 38 | [`demo38_im_channels`](examples/demo38_im_channels.rs) | IM 通道集成 |
| 39 | [`demo39_workflow`](examples/demo39_workflow.rs) | 图工作流引擎 |
| 40 | [`demo40_snapshot`](examples/demo40_snapshot.rs) | 快照与回滚 |
| 41 | [`demo41_web_tools`](examples/demo41_web_tools.rs) | Web 搜索 + 获取 |
| 42 | [`demo42_playwright_mcp`](examples/demo42_playwright_mcp.rs) | Playwright MCP 浏览器自动化 |
| 43 | [`demo43_data_tools`](examples/demo43_data_tools.rs) | Excel / CSV / Word / Text 数据处理 |
| 67 | [`demo67_progress`](examples/demo67_progress.rs) | 进度事件 |
| 68 | [`demo68_human_gate`](examples/demo68_human_gate.rs) | 人工审批门禁 |
| 70 | [`demo70_scheduler`](examples/demo70_scheduler.rs) | 任务调度 |

另有 **6 个综合示例** 展示真实场景应用：

| 示例 | 场景 |
|------|------|
| [`demo44_code_laboratory`](examples/demo44_code_laboratory.rs) | 代码执行助手 |
| [`demo45_customer_service`](examples/demo45_customer_service.rs) | 智能客服 |
| [`demo46_data_analyst`](examples/demo46_data_analyst.rs) | 数据分析助手 |
| [`demo47_enterprise`](examples/demo47_enterprise.rs) | 企业工作流自动化 |
| [`demo48_personal_assistant`](examples/demo48_personal_assistant.rs) | 个人智能助手 |
| [`demo49_research_agent`](examples/demo49_research_agent.rs) | 研究报告助手 |

---

## 兼容性

支持任意 **OpenAI 兼容** API，以及原生 Anthropic、Ollama：

| Provider | 接入地址 | 说明 |
|----------|---------|------|
| OpenAI | `https://api.openai.com/v1` | GPT-4o, GPT-4-turbo |
| Anthropic | `https://api.anthropic.com/v1` | 原生 Claude API |
| DeepSeek | `https://api.deepseek.com/v1` | DeepSeek-V3/R1 |
| 阿里云 Qwen | `https://dashscope.aliyuncs.com/compatible-mode/v1` | Qwen3-max, Qwen-plus |
| Ollama（本地） | `http://localhost:11434` | 原生协议 |
| LM Studio | `http://localhost:1234/v1` | 任意 GGUF 模型 |

---

## 文档

| 主题 | English | 中文 |
|------|---------|------|
| 面向贡献者的 Rust 教程 | — | [ZH](docs/zh/rust-learning/README.md) |
| ReAct Agent | [EN](docs/en/01-react-agent.md) | [ZH](docs/zh/01-react-agent.md) |
| 工具系统 | [EN](docs/en/02-tools.md) | [ZH](docs/zh/02-tools.md) |
| 记忆系统 | [EN](docs/en/03-memory.md) | [ZH](docs/zh/03-memory.md) |
| 上下文压缩 | [EN](docs/en/04-compression.md) | [ZH](docs/zh/04-compression.md) |
| 人工介入 | [EN](docs/en/05-human-loop.md) | [ZH](docs/zh/05-human-loop.md) |
| 多 Agent | [EN](docs/en/06-subagent.md) | [ZH](docs/zh/06-subagent.md) |
| Skill 系统 | [EN](docs/en/07-skills.md) | [ZH](docs/zh/07-skills.md) |
| MCP 协议 | [EN](docs/en/08-mcp.md) | [ZH](docs/zh/08-mcp.md) |
| DAG 任务 | [EN](docs/en/09-tasks.md) | [ZH](docs/zh/09-tasks.md) |
| 流式输出 | [EN](docs/en/10-streaming.md) | [ZH](docs/zh/10-streaming.md) |
| 结构化输出 | [EN](docs/en/11-structured-output.md) | [ZH](docs/zh/11-structured-output.md) |
| Mock 测试 | [EN](docs/en/12-mock.md) | [ZH](docs/zh/12-mock.md) |
| IM 通道 | [EN](docs/en/15-im-channels.md) | [ZH](docs/zh/15-im-channels.md) |
| 图工作流 | [EN](docs/en/17-graph-workflow.md) | [ZH](docs/zh/17-graph-workflow.md) |
| 护栏系统 | [EN](docs/en/18-guard-system.md) | [ZH](docs/zh/18-guard-system.md) |
| Web 工具 | [EN](docs/en/20-web-tools.md) | [ZH](docs/zh/20-web-tools.md) |
| 常用工具 | [EN](docs/en/21-common-tools.md) | [ZH](docs/zh/21-common-tools.md) |
| 论文检索 | [EN](docs/en/22-research-tools.md) | [ZH](docs/zh/22-research-tools.md) |
| Hooks 系统 | [EN](docs/en/23-hooks.md) | [ZH](docs/zh/23-hooks.md) |
| 评估系统 | [EN](docs/en/24-eval-system.md) | [ZH](docs/zh/24-eval-system.md) |
| 自进化系统 | [EN](docs/en/25-self-improvement.md) | [ZH](docs/zh/25-self-improvement.md) |
| 多 Agent 模式 | [EN](docs/en/26-multi-agent.md) | [ZH](docs/zh/26-multi-agent.md) |
| 追踪系统 | [EN](docs/en/27-tracing.md) | [ZH](docs/zh/27-tracing.md) |
| 配置参考 | [EN](docs/en/28-config-reference.md) | [ZH](docs/zh/28-config-reference.md) |
| 运行时与任务系统 | [EN](docs/en/29-long-running-tasks.md) | [ZH](docs/zh/29-long-running-tasks.md) |
| 安全指南 | [EN](docs/en/security.md) | [ZH](docs/zh/security.md) |

---

## 参与贡献

欢迎贡献！详见 [CONTRIBUTING.md](./CONTRIBUTING.md)。

```bash
git clone https://github.com/EchoYue-lp/echo-agent
cd echo-agent
cargo fmt --all
./scripts/verify.sh
```

---

## 更新日志

详见 [CHANGELOG.md](./CHANGELOG.md)。

---

## License

MIT &copy; echo-agent contributors


## Star History

<a href="https://www.star-history.com/?repos=EchoYue-lp%2Fecho-agent&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=EchoYue-lp/echo-agent&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=EchoYue-lp/echo-agent&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=EchoYue-lp/echo-agent&type=date&legend=top-left" />
 </picture>
</a>
