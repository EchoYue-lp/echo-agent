<div align="center">

# 🤖 echo-agent

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

#[tokio::main]
async fn main() -> Result<()> {
    let mut agent = ReactAgent::new(
        AgentConfig::new("qwen3-max", "助手", "你是一个有帮助的助手")
            .enable_tool(true)
    );
    agent.add_skill(Box::new(CalculatorSkill));
    agent.add_skill(Box::new(FileSystemSkill));

    let answer = agent.execute("计算 1337 * 42，并将结果保存到 result.txt").await?;
    println!("{answer}");
    Ok(())
}
```

---

## 功能一览

| 能力 | 描述 |
|------|------|
| 🔄 **ReAct 引擎** | Thought → Action → Observation 循环，内置 Chain-of-Thought |
| 🔧 **工具系统** | 实现 `Tool` trait，自动获得超时、重试、并行执行能力 |
| 🧠 **双层记忆** | `Store`（长期 KV 记忆）+ `Checkpointer`（会话历史）—— 对标 LangGraph 架构 |
| 📦 **上下文压缩** | 滑动窗口 / LLM 摘要 / 混合管道 —— 自动透明执行 |
| 🤝 **人工介入** | 工具审批门，支持命令行、Webhook、WebSocket 三种 Provider |
| 🏗️ **多 Agent 编排** | Orchestrator → SubAgent 分派，严格上下文隔离 |
| 💡 **Skill 系统** | 将工具 + 提示词片段打包为可复用的能力单元 |
| 🔌 **MCP 协议** | 接入任意符合 MCP 规范的工具服务（stdio 或 HTTP SSE） |
| 📊 **DAG 任务规划** | Planner 角色 + 拓扑调度 + 循环依赖检测 |
| 📡 **流式输出** | `execute_stream()` 返回 `AgentEvent` 流，实时推送 Token / 工具调用 |
| 📐 **结构化输出** | `extract::<T>()` / `extract_json()` —— 通过 JSON Schema 将 LLM 输出直接反序列化为 Rust 类型 |
| 🎣 **生命周期回调** | 监听每个执行阶段：推理、工具调用、最终答案、迭代轮次 |
| 🛡️ **容错与韧性** | 工具级超时、指数退避重试、并发数限流 |

---

## 架构图

```
┌─────────────────────────────────────────────────────────────────┐
│                          你的应用程序                             │
└────────────────────────────────┬────────────────────────────────┘
                                 │  execute() / execute_stream()
              ┌──────────────────▼──────────────────┐
              │            ReactAgent                │
              │                                      │
              │  ContextManager   ToolManager        │
              │  （自动压缩）      （超时/重试）       │
              │                                      │
              │  Store            Checkpointer        │
              │  （长期 KV）       （会话历史）        │
              │                                      │
              │  SubAgent 注册表   SkillManager       │
              │  人工审批管理器                        │
              └──────────────────┬──────────────────┘
                                 │  OpenAI 兼容 HTTP
              ┌──────────────────▼──────────────────┐
              │   LLM Provider（任意 OpenAI 兼容端）  │
              └─────────────────────────────────────┘
```

---

## 快速上手

### 前置条件

```bash
# 安装 Rust 工具链
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 项目配置

```toml
# Cargo.toml
[dependencies]
echo_agent = { path = "." }
tokio = { version = "1", features = ["full"] }
```

```bash
# .env
OPENAI_API_KEY=sk-...
OPENAI_BASE_URL=https://dashscope.aliyuncs.com/compatible-mode/v1  # 以阿里云 Qwen 为例
```

### 运行示例

```bash
cargo run --example demo01_tools
cargo run --example demo04_suagent
cargo run --example demo14_memory_isolation
```

---

## 核心概念速览

### 1. Tool —— 行动的原子单元

```rust
#[async_trait]
impl Tool for MyTool {
    fn name(&self)        -> &str   { "my_tool" }
    fn description(&self) -> &str   { "执行某项有用的操作" }
    fn parameters(&self)  -> Value  { json!({ /* JSON Schema */ }) }
    async fn execute(&self, params: ToolParameters) -> Result<ToolResult> {
        Ok(ToolResult::success("完成".to_string()))
    }
}

agent.add_tool(Box::new(MyTool));
```

### 2. Memory —— 两层记忆，两个问题

```rust
// 短期记忆：任何时候都能恢复对话
let config = AgentConfig::new(...)
    .session_id("user-alice-001")
    .checkpointer_path("./sessions.json");

// 长期记忆：知识跨越会话持续存在
let config = AgentConfig::new(...)
    .enable_memory(true)
    .memory_path("./knowledge.json");
// LLM 现在可以自主调用 remember / recall / forget 工具
```

### 3. Multi-Agent —— 将任务委派给专家

```rust
let mut orchestrator = ReactAgent::new(
    AgentConfig::new("qwen3-max", "总指挥", "将任务委派给合适的专家")
        .role(AgentRole::Orchestrator)
        .enable_subagent(true),
);
orchestrator.register_agents(vec![math_agent, research_agent, writer_agent]);
// 严格上下文隔离：每个 SubAgent 在独立的沙箱中运行
```

### 4. Streaming —— 实时反馈

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

### 5. MCP —— 接入任意工具服务器

```rust
let mut mcp = McpManager::new();
let tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx", vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
)).await?;
agent.add_tools(tools); // MCP 工具与本地工具完全一致
```

### 6. 结构化输出 —— LLM 响应直接反序列化为 Rust 结构体

```rust
#[derive(Debug, Deserialize)]
struct Invoice { vendor: String, amount: f64, date: String }

let invoice: Invoice = agent.extract(
    "收到 Acme 公司发票，金额 1250 元，日期 2025-03-15",
    ResponseFormat::json_schema("invoice", json!({
        "type": "object",
        "properties": {
            "vendor": { "type": "string" },
            "amount": { "type": "number" },
            "date":   { "type": "string" }
        },
        "required": ["vendor", "amount", "date"],
        "additionalProperties": false
    })),
).await?;
println!("{} 应付 ¥{:.2}", invoice.vendor, invoice.amount);
```

---

## 示例文件

| 示例 | 演示内容 |
|------|---------|
| [`demo01_tools`](examples/demo01_tools.rs) | 自定义工具注册与调用 |
| [`demo02_tasks`](examples/demo02_tasks.rs) | DAG 任务规划 |
| [`demo03_approval`](examples/demo03_approval.rs) | 人工审批门 |
| [`demo04_suagent`](examples/demo04_suagent.rs) | Orchestrator + Worker 模式 |
| [`demo05_compressor`](examples/demo05_compressor.rs) | 上下文压缩策略 |
| [`demo06_mcp`](examples/demo06_mcp.rs) | MCP 工具服务器接入 |
| [`demo07_skills`](examples/demo07_skills.rs) | 内置 Skill 安装 |
| [`demo08_external_skills`](examples/demo08_external_skills.rs) | 从 SKILL.md 加载外部技能 |
| [`demo09_file_shell`](examples/demo09_file_shell.rs) | 文件和 Shell 工具 |
| [`demo10_streaming`](examples/demo10_streaming.rs) | 实时流式输出 |
| [`demo11_callbacks`](examples/demo11_callbacks.rs) | 生命周期回调 |
| [`demo12_resilience`](examples/demo12_resilience.rs) | 重试、超时、容错 |
| [`demo13_tool_execution`](examples/demo13_tool_execution.rs) | 工具执行配置 |
| [`demo14_memory_isolation`](examples/demo14_memory_isolation.rs) | 记忆与上下文隔离 |
| [`demo15_structured_output`](examples/demo15_structured_output.rs) | 结构化输出（JSON Schema） |
| [`demo16_testing`](examples/demo16_testing.rs) | Mock 测试基础设施（零真实 LLM 调用） |

---

## 文档

完整文档位于 [`docs/`](./docs/)：

**中文**（[`docs/zh/`](./docs/zh/)）

- [ReAct Agent —— 核心执行引擎](docs/zh/01-react-agent.md)
- [工具系统](docs/zh/02-tools.md)
- [记忆系统（Store + Checkpointer）](docs/zh/03-memory.md)
- [上下文压缩](docs/zh/04-compression.md)
- [人工介入](docs/zh/05-human-loop.md)
- [多 Agent 编排](docs/zh/06-subagent.md)
- [Skill 系统](docs/zh/07-skills.md)
- [MCP 协议集成](docs/zh/08-mcp.md)
- [DAG 任务规划](docs/zh/09-tasks.md)
- [流式输出](docs/zh/10-streaming.md)
- [结构化输出](docs/zh/11-structured-output.md)
- [Mock 测试工具](docs/zh/12-mock.md)

**English**（[`docs/en/`](./docs/en/README.md)）：所有文档的英文版本。

---

## 兼容性

echo-agent 支持任意 **OpenAI 兼容** API 端点：

| Provider | 接入地址 |
|----------|---------|
| OpenAI | `https://api.openai.com/v1` |
| DeepSeek | `https://api.deepseek.com/v1` |
| 阿里云 Qwen | `https://dashscope.aliyuncs.com/compatible-mode/v1` |
| Ollama（本地） | `http://localhost:11434/v1` |
| LM Studio | `http://localhost:1234/v1` |
| 其他 | 设置 `OPENAI_BASE_URL` |

---

## 参与贡献

欢迎 PR 和 Issue！

```bash
git clone https://github.com/your-org/echo-agent
cd echo-agent
cargo build
cargo test
cargo run --example demo01_tools
```

**适合新手的入口：**
- 新增内置工具（参考 [`src/tools/others/`](src/tools/others/)）
- 新增内置 Skill（参考 [`src/skills/builtin/`](src/skills/builtin/)）
- 提升记忆模块的测试覆盖率

**提交 PR 前：**
- 运行 `cargo fmt` 和 `cargo clippy`
- 为新功能添加测试
- 更新 `docs/` 下相关文档

---

## License

MIT © echo-agent contributors
