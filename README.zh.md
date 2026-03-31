<div align="center">

# echo-agent

**为 Rust 打造的可组合、生产级 Agent 开发框架**

[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![OpenAI Compatible](https://img.shields.io/badge/API-OpenAI%20兼容-green)](https://platform.openai.com/docs/api-reference)
[![Async](https://img.shields.io/badge/async-tokio-blue)](https://tokio.rs/)

[English](./README.md) · [文档中心](docs/zh/README.md) · [示例](./examples/)

</div>

---

## 为什么选择 echo-agent？

绝大多数 AI Agent 框架基于 Python 构建。echo-agent 将完整的现代 Agent 框架能力带入 Rust 生态，让你同时享有：**内存安全**、**零成本抽象**、以及无可比拟的**异步并发性能**。

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

---

## 功能一览

| 能力 | 描述 |
|------|------|
| 🔄 **ReAct 引擎** | Thought → Action → Observation 循环，内置 Chain-of-Thought |
| 🔧 **工具系统** | `#[tool]` 宏 + 自动 JSON Schema；超时、重试、并行执行 |
| 🧠 **双层记忆** | `Store`（长期 KV）+ `Checkpointer`（会话历史） |
| 📦 **上下文压缩** | 滑动窗口 / LLM 摘要 / 混合管道 |
| 🤝 **人工介入** | 工具审批门，支持命令行、Webhook、WebSocket |
| 🏗️ **多 Agent 编排** | Orchestrator → SubAgent 分派；Agent 间 Handoff |
| 💡 **Skill 系统** | 工具 + 提示词打包为可复用的能力单元 |
| 🔌 **MCP 协议** | 接入任意 MCP 服务（stdio / SSE / HTTP） |
| 📊 **Plan-and-Execute** | Planner + Executor：先规划后执行策略 |
| 📡 **流式输出** | `execute_stream()` 返回实时 `AgentEvent` 流 |
| 📐 **结构化输出** | `extract::<T>()` —— 通过 JSON Schema 将 LLM 输出反序列化为 Rust 类型 |
| 🛡️ **护栏系统** | 规则 / LLM 双模式内容过滤 |
| 🔑 **权限模型** | 声明式工具权限 + 可插拔策略 |
| 📝 **审计日志** | 结构化事件日志，可插拔存储后端 |
| 📈 **OpenTelemetry** | 分布式追踪与指标（OTLP） |
| 🎣 **宏系统** | `#[tool]`、`#[callback]`、`#[guard]`、`#[handler]`、`agent!{}`、`messages![]` |
| 🌐 **A2A 协议** | Agent Card 发布 / 跨框架协作 |

---

## Workspace 结构

echo-agent 采用多 crate workspace 组织：

```
echo-agent/
├── echo-core/         核心 trait 与类型（Tool、LlmClient、Agent、Guard、Error 等）
├── echo-macros/       过程宏（#[tool]、#[callback]、#[guard]、#[handler] 等）
├── echo-providers/    LLM 提供方实现（OpenAI、Anthropic、Ollama）
├── echo-mcp/          MCP 协议客户端（stdio、SSE、HTTP 传输）
├── src/               主 crate —— Agent 引擎、记忆、技能、工具等
├── examples/          25 个可运行示例
├── docs/              双语文档（en + zh）
└── skills/            外部技能包（Markdown 格式）
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
```

---

## 核心概念

### 1. Tool —— 一个宏定义一个工具

```rust
use echo_agent::{tool, prelude::*};

#[tool(name = "weather", description = "查询城市天气")]
async fn weather(
    /// 城市名
    city: String,
) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{city} 晴天")))
}

// 自动生成: WeatherParams + WeatherTool + impl TypedTool
```

### 2. Agent —— 声明式构建

```rust
use echo_agent::{agent, prelude::*};

let mut agent = agent! {
    model: "qwen3-max",
    system_prompt: "你是一个有帮助的助手",
    tools: [WeatherTool, CalculatorTool],
    max_iterations: 10,
}?;

let answer = agent.execute("东京天气如何？").await?;
```

### 3. Callback —— 只覆写你需要的方法

```rust
use echo_agent::{callback, prelude::*};

struct MyCallback;

#[callback]
impl MyCallback {
    async fn on_tool_start(&self, _agent: &str, tool: &str, _args: &serde_json::Value) {
        println!("[工具调用] {tool}");
    }
}
```

### 4. Guard —— 一个函数即护栏

```rust
use echo_agent::{guard, prelude::*};

#[guard(name = "length-limit")]
async fn check_length(content: &str, direction: GuardDirection) -> Result<GuardResult> {
    if content.len() > 50000 {
        Ok(GuardResult::Block { reason: "内容过长".into() })
    } else {
        Ok(GuardResult::Pass)
    }
}
```

### 5. Streaming —— 实时反馈

```rust
let mut stream = agent.execute_stream("解释量子纠缠").await?;
while let Some(event) = stream.next().await {
    match event? {
        AgentEvent::Token(t)              => print!("{t}"),
        AgentEvent::ToolCall { name, .. } => println!("\n[→ {name}]"),
        AgentEvent::FinalAnswer(a)        => { println!("\n{a}"); break; }
        _ => {}
    }
}
```

### 6. MCP —— 接入任意工具服务器

```rust
let mut mcp = McpManager::new();
let tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx", vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
)).await?;
agent.add_tools(tools);
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

**中文**（[`docs/zh/`](./docs/zh/)）

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

**English**（[`docs/en/`](./docs/en/README.md)）

---

## 参与贡献

```bash
git clone https://github.com/your-org/echo-agent
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
