<div align="center">

# 🤖 echo-agent

**A composable, production-ready Agent framework for Rust**

[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![OpenAI Compatible](https://img.shields.io/badge/API-OpenAI%20Compatible-green)](https://platform.openai.com/docs/api-reference)
[![Async](https://img.shields.io/badge/async-tokio-blue)](https://tokio.rs/)

[中文文档](./README.zh.md) · [Documentation](./docs/en/README.md) · [Examples](./examples/)

</div>

---

## Why echo-agent?

Most AI agent frameworks are written in Python. echo-agent brings the full power of a modern agent framework to Rust — with **memory safety**, **zero-cost abstractions**, and **async-native concurrency** you can't get elsewhere.

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let mut agent = ReactAgent::new(
        AgentConfig::new("gpt-4o", "assistant", "You are a helpful assistant")
            .enable_tool(true)
    );
    agent.add_skill(Box::new(CalculatorSkill));
    agent.add_skill(Box::new(FileSystemSkill));

    let answer = agent.execute("Calculate 1337 * 42 and save it to result.txt").await?;
    println!("{answer}");
    Ok(())
}
```

---

## Features

| Capability | Description |
|------------|-------------|
| 🔄 **ReAct Engine** | Thought → Action → Observation loop with Chain-of-Thought |
| 🔧 **Tool System** | Implement `Tool` trait, get timeout + retry + parallel execution for free |
| 🧠 **Dual-layer Memory** | `Store` (long-term KV) + `Checkpointer` (session history) — mirrors LangGraph's architecture |
| 📦 **Context Compression** | SlidingWindow / LLM Summary / Hybrid pipeline — automatic, transparent |
| 🤝 **Human-in-the-Loop** | Approval gates with Console, Webhook, or WebSocket providers |
| 🏗️ **Multi-Agent Orchestration** | Orchestrator → SubAgent dispatch with strict context isolation |
| 💡 **Skill System** | Package tools + prompt fragments into reusable capability units |
| 🔌 **MCP Protocol** | Connect any MCP-compliant tool server (stdio or HTTP SSE) |
| 📊 **DAG Task Planning** | Planner role with topological scheduling and cycle detection |
| 📡 **Streaming Output** | `execute_stream()` returns `AgentEvent` stream with real-time Token/ToolCall events |
| 📐 **Structured Output** | `extract::<T>()` / `extract_json()` — LLM output directly deserialized into Rust types via JSON Schema |
| 🎣 **Lifecycle Callbacks** | Hook into every phase: think, tool call, final answer, iteration |
| 🛡️ **Resilience** | Per-tool timeout, exponential backoff retry, concurrency limits |

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         Your Application                         │
└────────────────────────────────┬────────────────────────────────┘
                                 │  execute() / execute_stream()
              ┌──────────────────▼──────────────────┐
              │            ReactAgent                │
              │                                      │
              │  ContextManager   ToolManager        │
              │  (auto-compress)  (timeout/retry)    │
              │                                      │
              │  Store            Checkpointer        │
              │  (long-term KV)   (session history)  │
              │                                      │
              │  SubAgent Registry   SkillManager    │
              │  HumanApprovalManager                │
              └──────────────────┬──────────────────┘
                                 │  OpenAI-compatible HTTP
              ┌──────────────────▼──────────────────┐
              │   LLM Provider (any OpenAI-compat)   │
              └─────────────────────────────────────┘
```

---

## Quick Start

### Prerequisites

```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Setup

```toml
# Cargo.toml
[dependencies]
echo_agent = { path = "." }
tokio = { version = "1", features = ["full"] }
```

```bash
# .env
OPENAI_API_KEY=sk-...
OPENAI_BASE_URL=https://api.openai.com/v1
# Or any OpenAI-compatible endpoint (Qwen, DeepSeek, Ollama, etc.)
```

### Run an example

```bash
cargo run --example demo01_tools
cargo run --example demo04_suagent
cargo run --example demo14_memory_isolation
```

---

## Core Concepts

### 1. Tool — the atomic unit of action

```rust
#[async_trait]
impl Tool for MyTool {
    fn name(&self)        -> &str   { "my_tool" }
    fn description(&self) -> &str   { "Does something useful" }
    fn parameters(&self)  -> Value  { json!({ /* JSON Schema */ }) }
    async fn execute(&self, params: ToolParameters) -> Result<ToolResult> {
        Ok(ToolResult::success("done".to_string()))
    }
}

agent.add_tool(Box::new(MyTool));
```

### 2. Memory — two layers, two problems

```rust
// Short-term: resume any conversation, any time
let config = AgentConfig::new(...)
    .session_id("user-alice-001")
    .checkpointer_path("./sessions.json");

// Long-term: knowledge that outlives sessions
let config = AgentConfig::new(...)
    .enable_memory(true)
    .memory_path("./knowledge.json");
// LLM can now call remember / recall / forget tools autonomously
```

### 3. Multi-Agent — delegate to specialists

```rust
let mut orchestrator = ReactAgent::new(
    AgentConfig::new("gpt-4o", "boss", "Delegate tasks to the right specialist")
        .role(AgentRole::Orchestrator)
        .enable_subagent(true),
);
orchestrator.register_agents(vec![math_agent, research_agent, writer_agent]);
// Strict context isolation: each SubAgent runs in its own sandbox
```

### 4. Streaming — real-time feedback

```rust
let mut stream = agent.execute_stream("Explain quantum entanglement").await?;
while let Some(event) = stream.next().await {
    match event? {
        AgentEvent::Token(t)          => print!("{t}"),
        AgentEvent::ToolCall { name, ..} => println!("\n[→ {name}]"),
        AgentEvent::FinalAnswer(a)    => { println!("\n{a}"); break; }
        _ => {}
    }
}
```

### 5. MCP — plug in any tool server

```rust
let mut mcp = McpManager::new();
let tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx", vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
)).await?;
agent.add_tools(tools); // MCP tools look identical to native tools
```

### 6. Structured Output — LLM responses as typed Rust structs

```rust
#[derive(Debug, Deserialize)]
struct Invoice { vendor: String, amount: f64, date: String }

let invoice: Invoice = agent.extract(
    "Invoice from Acme Corp, $1,250.00, dated 2025-03-15",
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
println!("{} owes ${:.2}", invoice.vendor, invoice.amount);
```

---

## Examples

| Example | What it demonstrates |
|---------|---------------------|
| [`demo01_tools`](examples/demo01_tools.rs) | Register and invoke custom tools |
| [`demo02_tasks`](examples/demo02_tasks.rs) | DAG task planning with a Planner Agent |
| [`demo03_approval`](examples/demo03_approval.rs) | Human-in-the-loop approval gate |
| [`demo04_suagent`](examples/demo04_suagent.rs) | Orchestrator + Worker SubAgent pattern |
| [`demo05_compressor`](examples/demo05_compressor.rs) | Context compression strategies |
| [`demo06_mcp`](examples/demo06_mcp.rs) | Connecting an MCP tool server |
| [`demo07_skills`](examples/demo07_skills.rs) | Installing built-in Skills |
| [`demo08_external_skills`](examples/demo08_external_skills.rs) | Loading Skills from SKILL.md files |
| [`demo09_file_shell`](examples/demo09_file_shell.rs) | File and shell tools |
| [`demo10_streaming`](examples/demo10_streaming.rs) | Real-time streaming output |
| [`demo11_callbacks`](examples/demo11_callbacks.rs) | Lifecycle callbacks |
| [`demo12_resilience`](examples/demo12_resilience.rs) | Retry, timeout, fault tolerance |
| [`demo13_tool_execution`](examples/demo13_tool_execution.rs) | Tool execution configuration |
| [`demo14_memory_isolation`](examples/demo14_memory_isolation.rs) | Memory + context isolation demo |
| [`demo15_structured_output`](examples/demo15_structured_output.rs) | Structured output with JSON Schema |
| [`demo16_testing`](examples/demo16_testing.rs) | Mock testing infrastructure — zero real LLM calls |

---

## Documentation

Full documentation lives in [`docs/en/`](./docs/en/README.md):

- [ReAct Agent — core execution engine](docs/en/01-react-agent.md)
- [Tool System](docs/en/02-tools.md)
- [Memory System (Store + Checkpointer)](docs/en/03-memory.md)
- [Context Compression](docs/en/04-compression.md)
- [Human-in-the-Loop](docs/en/05-human-loop.md)
- [Multi-Agent Orchestration](docs/en/06-subagent.md)
- [Skill System](docs/en/07-skills.md)
- [MCP Protocol Integration](docs/en/08-mcp.md)
- [DAG Task Planning](docs/en/09-tasks.md)
- [Streaming Output](docs/en/10-streaming.md)
- [Structured Output](docs/en/11-structured-output.md)
- [Mock Testing Utilities](docs/en/12-mock.md)


---

## Compatibility

echo-agent works with any **OpenAI-compatible** API endpoint:

| Provider | Base URL |
|----------|---------|
| OpenAI | `https://api.openai.com/v1` |
| DeepSeek | `https://api.deepseek.com/v1` |
| Alibaba Qwen | `https://dashscope.aliyuncs.com/compatible-mode/v1` |
| Ollama (local) | `http://localhost:11434/v1` |
| LM Studio | `http://localhost:1234/v1` |
| Any other | Set `OPENAI_BASE_URL` |

---

## Contributing

Contributions are welcome! Here's how to get started:

```bash
git clone https://github.com/your-org/echo-agent
cd echo-agent
cargo build
cargo test
cargo run --example demo01_tools
```

**Good first issues:**
- Add a new built-in tool (see [`src/tools/others/`](src/tools/others/))
- Add a new built-in Skill (see [`src/skills/builtin/`](src/skills/builtin/))
- Improve test coverage for memory modules

**Before submitting a PR:**
- Run `cargo fmt` and `cargo clippy`
- Add tests for new functionality
- Update relevant docs in `docs/`

---

## License

MIT © echo-agent contributors
