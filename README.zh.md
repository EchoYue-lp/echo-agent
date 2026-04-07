<div align="center">

# echo-agent

**生产级、可组合的 Rust AI Agent 开发框架**

[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-1.0.0-brightgreen)](https://github.com/your-org/echo-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![OpenAI Compatible](https://img.shields.io/badge/API-OpenAI%20Compatible-green)](https://platform.openai.com/docs/api-reference)
[![Async](https://img.shields.io/badge/runtime-tokio-blue)](https://tokio.rs/)

用 Rust 的 **内存安全**、**零成本抽象** 和 **原生异步并发** 构建自主 AI Agent。

[English](./README.md) · [文档](./docs/zh/README.md) · [示例](./examples/) · [知识库](./docs/knowledge/)

</div>

---

## 为什么选择 echo-agent？

大多数 AI Agent 框架用 Python 编写。**echo-agent** 将现代 Agent 框架的完整能力带入 Rust —— 与 LangGraph、CrewAI、AutoGen 功能对等，同时提供只有 Rust 才能提供的性能和可靠性。

### Rust 的优势

| Python 框架 | echo-agent (Rust) |
|-------------|-------------------|
| 拼写错误在运行时才发现 | 编译时类型检查 |
| GIL 限制并发 | 真正的异步并行 |
| 可能的内存泄漏 | 保证内存安全 |
| 启动慢（模块导入） | 二进制即时启动 |
| 部署复杂 | 单文件部署 |

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
        system_prompt: "你是一个数学助手",
        tools: [AddTool],
    }?;

    let answer = agent.execute("计算 1337 × 42").await?;
    println!("{answer}");
    Ok(())
}
```

---

## 项目亮点

| 指标 | 数值 |
|------|------|
| **能力模块** | 30+ 模块：ReAct、工具、记忆、流式、多Agent、技能、MCP、护栏、审计... |
| **示例程序** | 39 个可运行 demo — 每个功能都有 `cargo run --example demoXX` |
| **单元测试** | 350+ 测试用例，覆盖率完善 |
| **Crate 结构** | 5 个模块化 crate，1 行导入 `use echo_agent::prelude::*` |
| **源文件** | 154 个 Rust 源文件 |
| **文档** | 中英双语 + 18 篇功能指南 |

---

## 架构总览

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          用户 / 应用层                                    │
└────────────────────────────────────┬────────────────────────────────────┘
                                     │
                                     │  execute()     → 单次任务（重置上下文）
                                     │  chat()        → 多轮对话（保留历史）
                                     │  execute_stream() / chat_stream() → 实时事件流
                                     │
┌────────────────────────────────────▼────────────────────────────────────┐
│                         ReactAgent 执行引擎                              │
│                                                                         │
│   ┌─────────────────┐   ┌─────────────────┐   ┌─────────────────────┐  │
│   │ ContextManager  │   │   ToolManager   │   │    SkillRegistry    │  │
│   │ (压缩/截断)     │   │ (注册/执行)     │   │ (代码+文件技能)     │  │
│   └─────────────────┘   └─────────────────┘   └─────────────────────┘  │
│                                                                         │
│   ┌─────────────────┐   ┌─────────────────┐   ┌─────────────────────┐  │
│   │   Checkpointer  │   │     Store       │   │   GuardManager      │  │
│   │ (会话历史持久化)│   │  (长期KV存储)   │   │  (输入/输出过滤)    │  │
│   └─────────────────┘   └─────────────────┘   └─────────────────────┘  │
│                                                                         │
│   ┌─────────────────────────────────────────────────────────────────┐  │
│   │                     SubAgent 注册表                              │  │
│   │   orchestrator → math_agent / writer_agent / researcher_agent   │  │
│   └─────────────────────────────────────────────────────────────────┘  │
│                                                                         │
│   ┌─────────────────┐   ┌─────────────────┐   ┌─────────────────────┐  │
│   │  AuditLogger    │   │  PermissionSvc  │   │    McpManager       │  │
│   │ (事件审计)      │   │ (工具权限)      │   │  (外部工具)         │  │
│   └─────────────────┘   └─────────────────┘   └─────────────────────┘  │
└────────────────────────────────────┬────────────────────────────────────┘
                                     │
                                     │  HTTP (OpenAI 兼容 API)
                                     │
┌────────────────────────────────────▼────────────────────────────────────┐
│                         LLM 提供商层                                     │
│                                                                         │
│   ┌──────────────┐   ┌──────────────┐   ┌──────────────┐   ┌─────────┐ │
│   │   OpenAI     │   │  Anthropic   │   │   Ollama     │   │  Qwen   │ │
│   │  (GPT-4o)    │   │  (Claude)    │   │  (本地)      │   │(DeepSeek)│ │
│   └──────────────┘   └──────────────┘   └──────────────┘   └─────────┘ │
│                                                                         │
│   统一 RetryPolicy + ProviderFactory 支持热切换                         │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 核心特性

### Agent 执行策略

| 策略 | 模式 | 适用场景 |
|------|------|----------|
| **ReAct** | 思考 → 行动 → 观察 | 交互式推理、工具编排 |
| **Plan-and-Execute** | 规划器 → 执行器 → 汇总 | 结构化多步任务（DAG） |
| **Self-Reflection** | 生成 → 评估 → 修正 | 质量保证输出、持续学习 |

### 双层记忆系统

| 层级 | Trait | 存储 | 作用域 |
|------|-------|------|--------|
| **短期** | `Checkpointer` | File / InMemory | 会话对话历史 |
| **长期** | `Store` | File / InMemory / SQLite / Embedding | 命名空间隔离的 KV 存储 |
| **语义** | `EmbeddingStore` | 向量索引 | 余弦相似度检索 |

### 工具系统

```rust
// 一个宏 = 参数结构体 + JSON Schema + TypedTool 实现
#[tool(name = "search_web", description = "搜索网页信息")]
async fn search_web(
    /// 搜索关键词
    query: String,
    /// 最大结果数（默认 5）
    #[schemars(default = "default_limit")]
    limit: usize,
) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("搜索结果: {}", query)))
}

fn default_limit() -> usize { 5 }
```

**能力：**
- 自动 JSON Schema 生成（schemars）
- 超时 + 重试 + 并发限制（`ToolExecutionConfig`）
- 动态工具管理（`add_tool`、`remove_tool`、`replace_tool`）
- 可插拔的权限策略

### 图工作流（对标 LangGraph）

```rust
use echo_agent::workflow::{GraphBuilder, SharedState};

let graph = GraphBuilder::new("research_pipeline")
    // Agent 节点：输入/输出键映射
    .add_agent_node("researcher", researcher_agent)
        .input_key("task")
        .output_key("research")
    .add_agent_node("writer", writer_agent)
        .input_key("research")
        .output_key("result")
    // 函数节点：数据转换
    .add_function_node("format", |state| Box::pin(async move {
        let result: String = state.get("result").unwrap_or_default();
        state.set("final", format!("### 报告\n\n{}", result));
        Ok(())
    }))
    // 图结构定义
    .set_entry("researcher")
    .add_edge("researcher", "writer")
    .add_edge("writer", "format")
    .set_finish("format")
    .build()?;

// 执行并获取实时事件
let mut stream = graph.run_stream(SharedState::new()).await?;
while let Some(event) = stream.next().await {
    match event? {
        WorkflowEvent::NodeStart { node_name, .. } => println!("▶ {node_name}"),
        WorkflowEvent::NodeEnd { elapsed, .. } => println!("✓ 完成 ({elapsed:?})"),
        WorkflowEvent::Completed { result, .. } => println!("最终: {result}"),
        _ => {}
    }
}
```

**工作流类型：**
- `Graph` — LangGraph 风格 DAG，支持条件边
- `SequentialWorkflow` — 简单管道
- `ConcurrentWorkflow` — 并行执行
- `DagWorkflow` — 拓扑调度

### MCP 协议集成

```rust
use echo_agent::mcp::{McpManager, McpServerConfig};

let mut mcp = McpManager::new();

// 连接 stdio MCP 服务器（文件系统访问）
let tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx", vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
)).await?;

agent.add_tools(tools);

// 连接 HTTP MCP 服务器
let tools = mcp.connect(McpServerConfig::http(
    "api-server",
    "http://localhost:3000/mcp"
)).await?;
```

**传输支持：**
- stdio（本地进程）
- SSE（Server-Sent Events）
- HTTP（REST API）

### Skill 系统（对齐 agentskills.io）

```rust
// 代码型 Skill：工具包 + 可选提示词注入
pub struct CalculatorSkill;

impl Skill for CalculatorSkill {
    fn name(&self) -> &str { "calculator" }
    fn description(&self) -> &str { "数学计算" }
    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![Box::new(AddTool), Box::new(MultiplyTool)]
    }
    fn system_prompt_injection(&self) -> Option<String> {
        Some("你有计算器用于精确数学运算。".into())
    }
}

// 文件型 Skill（SKILL.md）：
// skills/code_review/SKILL.md
// → 渐进式披露：发现 → 激活 → 使用
```

### Human-in-the-Loop

```rust
use echo_agent::human_loop::{ConsoleHumanLoopProvider, WebSocketHumanLoopProvider};

// 控制台审批（终端提示）
let provider = ConsoleHumanLoopProvider::new();
agent.set_human_loop_provider(Arc::new(provider));

// WebSocket 审批（Web UI）
let provider = WebSocketHumanLoopProvider::new("ws://localhost:8080/approval");
agent.set_human_loop_provider(Arc::new(provider));
```

### 护栏系统

```rust
// 规则型护栏（即时）
let guard = RuleGuardBuilder::new("no-pii")
    .block_regex(r"\b\d{3}-\d{2}-\d{4}\b") // SSN 模式
    .block_regex(r"\b[A-Z]{2}\d{6}\b")     // 护照模式
    .build();

// LLM 型护栏（语义理解）
let llm_guard = LlmGuard::new("qwen3-max")
    .prompt("检查内容是否包含敏感信息");

agent.set_guard_manager(GuardManager::new()
    .add_input_guard(Box::new(guard))
    .add_output_guard(Box::new(llm_guard)));
```

### 结构化输出

```rust
#[derive(Deserialize, JsonSchema)]
struct AnalysisResult {
    sentiment: String,
    confidence: f64,
    keywords: Vec<String>,
}

let result: AnalysisResult = agent.extract("分析：'我喜欢 Rust！'").await?;
println!("情感: {} ({:.0}% 置信度)", result.sentiment, result.confidence * 100);
```

### Self-Reflection Agent

```rust
use echo_agent::agents::self_reflection::{SelfReflectionAgent, LlmCritic};

let generator = ReactAgentBuilder::simple("qwen3-max", "技术文档撰写者")?;
let critic = LlmCritic::new("qwen3-max").with_pass_threshold(8.0);

let mut agent = SelfReflectionAgent::new("reflection_agent", generator, critic)
    .max_reflections(3);

// 生成 → 评估（分数 < 8）→ 反思 → 修正 → 循环
let result = agent.execute("清晰准确地解释 Rust 所有权").await?;
```

### Plan-and-Execute Agent

```rust
use echo_agent::agents::plan_execute::{PlanExecuteAgent, LlmPlanner, ReactExecutor};

let planner = LlmPlanner::new("qwen3-max");
let executor = ReactExecutor::new(ReactAgentBuilder::simple("qwen3-max", "执行器")?);

let mut agent = PlanExecuteAgent::new("planner", planner, executor)
    .max_replans(3);

// 规划 → DAG 任务 → 执行 → 失败时增量重规划 → 汇总
let result = agent.execute("研究、分析并撰写 Rust 异步模式报告").await?;
```

### A2A 协议（Agent-to-Agent）

```rust
use echo_agent::a2a::{AgentCard, A2AServer, A2AClient};

// 服务端：发布 Agent Card
let card = AgentCard::builder("translator", "http://localhost:8080")
    .description("多语言翻译 Agent")
    .skill(AgentSkill::new("translate", "翻译文本"))
    .streaming()
    .build();

let server = A2AServer::new(card, agent);

// 客户端：发现并调用远程 Agent
let client = A2AClient::new("http://remote-agent:8080");
let task_id = client.send_task("翻译成法语：你好世界").await?;
```

---

## 宏系统

| 宏 | 输入 | 输出 | 用途 |
|-----|------|------|------|
| `#[tool]` | `async fn` | `Params` + `TypedTool` | 一个函数定义工具 |
| `#[callback]` | `impl` 块 | `AgentCallback` | 覆写生命周期钩子 |
| `#[guard]` | `async fn` | `Guard` | 内容过滤规则 |
| `#[handler]` | `impl` 块 | `HumanLoopHandler` | 审批/输入处理 |
| `#[compressor]` | `async fn` | `ContextCompressor` | 自定义压缩策略 |
| `#[audit_logger]` | `impl` 块 | `AuditLogger` | 事件日志后端 |
| `agent!{}` | 键值对 | `ReactAgent` | 声明式 Agent 构建 |
| `messages![]` | 角色-内容对 | `Vec<Message>` | 快速消息列表 |
| `tool_params!{}` | schema DSL | `JSON Value` | JSON Schema 构建器 |

---

## 工作区结构

```
echo-agent/
├── echo-core/          # 核心 trait 与类型
│   ├── agent.rs        # Agent trait, AgentEvent, AgentCallback
│   ├── tools/mod.rs    # Tool, TypedTool, ToolResult, Permission
│   ├── llm/mod.rs      # LlmClient trait, Message 类型
│   ├── guard.rs        # Guard trait, GuardResult
│   ├── audit.rs        # AuditLogger trait
│   └── retry.rs        # RetryPolicy, with_retry, with_retry_if
│
├── echo-macros/        # 过程宏
│   └── lib.rs          # #[tool], #[callback], #[guard], #[handler] 等
│
├── echo-providers/     # LLM 实现
│   ├── openai.rs       # OpenAI 兼容客户端
│   ├── anthropic.rs    # Anthropic Claude 原生客户端
│   └── ollama.rs       # 本地 Ollama 客户端
│
├── echo-mcp/           # MCP 协议
│   ├── client.rs       # McpManager, 工具适配
│   ├── transport/      # stdio, SSE, HTTP 传输
│   └── types.rs        # MCP 消息类型
│
├── src/                # 主 crate
│   ├── agents/         # Agent 实现
│   │   ├── react/      # ReAct 引擎
│   │   ├── plan_execute/ # Plan-and-Execute
│   │   ├── self_reflection/ # Self-Reflection
│   │   └── subagent/   # SubAgent 编排
│   ├── memory/         # Store, Checkpointer, EmbeddingStore
│   ├── workflow/       # Graph, Sequential, Concurrent, DagWorkflow
│   ├── tools/          # ToolManager, 内置工具
│   ├── skills/         # SkillRegistry, Skill trait
│   ├── guard/          # GuardManager, RuleGuard, LlmGuard
│   ├── compression/    # SlidingWindow, Summary, Hybrid
│   ├── testing/        # MockLlmClient, MockAgent, MockTool
│   ├── a2a/            # Agent-to-Agent 协议
│   └── telemetry/      # OpenTelemetry 集成
│
├── examples/           # 39 个可运行 demo
├── docs/               # 中英双语文档
│   └ knowledge/        # 知识库（模式、概念）
├── skills/             # 外部技能包（SKILL.md 格式）
│
└── echo-cli/           # CLI 工具（可选）
```

---

## 快速开始

### 环境准备

```bash
# 安装 Rust（2024 edition）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 配置

创建 `echo-agent.yaml`（或设置 `$ECHO_AGENT_CONFIG`）：

```yaml
models:
  qwen3-max:
    base_url: https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions
    api_key: ${DASHSCOPE_API_KEY}

  gpt-4o:
    provider: openai
    api_key: ${OPENAI_API_KEY}

  claude-3-5-sonnet:
    provider: anthropic
    api_key: ${ANTHROPIC_API_KEY}

  llama3:
    provider: ollama
    base_url: http://localhost:11434
```

### 运行示例

```bash
# 基础工具
cargo run --example demo01_tools

# 宏展示
cargo run --example demo25_macros

# 工作流流式
cargo run --example demo34_workflow_stream

# 多模态（图片）
cargo run --example demo36_multimodal

# 声明式 YAML 工作流
cargo run --example demo37_declarative_workflow

# Self-Reflection Agent
cargo run --example demo20_audit

# MCP 集成
cargo run --example demo06_mcp
```

### 构建 & 测试

```bash
cargo build --release
cargo test --lib
cargo clippy -- -D warnings
```

---

## 文档

### 功能指南

| 文档 | 模块 | 核心概念 |
|------|------|----------|
| [01 - ReAct Agent](docs/zh/01-react-agent.md) | 核心引擎 | 思考→行动→观察、CoT、回调 |
| [02 - 工具系统](docs/zh/02-tools.md) | Tools | TypedTool、超时重试、权限 |
| [03 - 记忆系统](docs/zh/03-memory.md) | Memory | Store、Checkpointer、命名空间隔离 |
| [04 - 上下文压缩](docs/zh/04-compression.md) | Compression | SlidingWindow、Summary、Hybrid |
| [05 - 人工介入](docs/zh/05-human-loop.md) | HIL | 审批、Console/WebSocket 提供者 |
| [06 - 多 Agent](docs/zh/06-subagent.md) | SubAgent | Orchestrator、上下文隔离 |
| [07 - Skill 系统](docs/zh/07-skills.md) | Skills | 代码/文件型、渐进式披露 |
| [08 - MCP 集成](docs/zh/08-mcp.md) | MCP | stdio/HTTP 传输、工具适配 |
| [09 - 任务规划](docs/zh/09-tasks.md) | Tasks | DAG、拓扑排序、Mermaid |
| [10 - 流式输出](docs/zh/10-streaming.md) | Streaming | AgentEvent、SSE、TTFT |
| [11 - 结构化输出](docs/zh/11-structured-output.md) | Structured | JsonSchema、extract() |
| [12 - Mock 测试](docs/zh/12-mock.md) | Testing | MockLlmClient、MockAgent |
| [13 - 多轮对话](docs/zh/13-chat.md) | Chat | chat()、reset() |
| [14 - 语义搜索](docs/zh/14-semantic-search.md) | Semantic | EmbeddingStore、向量 |
| [15 - Self-Reflection](docs/zh/15-self-reflection.md) | 反思 | 评估→修正、情景记忆 |
| [16 - Plan-and-Execute](docs/zh/16-plan-execute.md) | 规划执行 | Planner/Executor、增量重规划 |
| [17 - Graph Workflow](docs/zh/17-graph-workflow.md) | 工作流 | LangGraph 风格、条件边 |
| [18 - Guard 系统](docs/zh/18-guard-system.md) | 护栏 | RuleGuard、LlmGuard |

### 知识库

| 主题 | 描述 |
|------|------|
| [Agent 模式](docs/knowledge/agent-patterns.md) | ReAct、Plan-and-Execute、Self-Reflection、LangGraph 工作流 |
| [MCP 协议](docs/knowledge/mcp-protocol.md) | Model Context Protocol 规范与集成 |
| [Skill 系统设计](docs/knowledge/skill-system.md) | agentskills.io 规范与渐进式披露 |
| [A2A 协议](docs/knowledge/a2a-protocol.md) | Agent-to-Agent 通信与发现 |

---

## 提供商兼容性

| 提供商 | 端点 | 特性 |
|--------|------|------|
| OpenAI | `https://api.openai.com/v1` | GPT-4o、流式、视觉 |
| Anthropic | `https://api.anthropic.com/v1` | Claude、原生格式 |
| DeepSeek | `https://api.deepseek.com/v1` | DeepSeek-V3 |
| 阿里 Qwen | `https://dashscope.aliyuncs.com/compatible-mode/v1` | Qwen3-max |
| Ollama (本地) | `http://localhost:11434` | Llama3、Mistral、Qwen2 |
| LM Studio | `http://localhost:1234/v1` | 本地模型 |

---

## Feature Flags

```toml
[dependencies]
echo_agent = { version = "1.0.0", features = ["full"] }

# 或选择性启用：
echo_agent = { version = "1.0.0", features = ["mcp", "a2a", "sqlite"] }
```

| Feature | 描述 |
|---------|------|
| `full` | 所有功能（默认） |
| `mcp` | MCP 协议集成 |
| `a2a` | Agent-to-Agent 协议 |
| `sqlite` | SQLite 存储 |
| `telemetry` | OpenTelemetry 追踪 |
| `human-loop` | WebSocket 审批 |
| `plan-execute` | Plan-and-Execute Agent |
| `workflow` | 图工作流引擎 |
| `self-reflection` | Self-Reflection Agent |
| `subagent` | SubAgent 编排 |

---

## 贡献指南

```bash
git clone https://github.com/your-org/echo-agent
cd echo-agent
cargo build
cargo test --lib
cargo run --example demo01_tools
```

提交 PR 前：
- `cargo fmt && cargo clippy -- -D warnings`
- 为新功能添加测试
- 更新 `docs/` 中的文档

---

## 许可证

MIT © echo-agent 贡献者