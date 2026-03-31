<div align="center">

# echo-agent

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
use echo_agent::{agent, tool};

#[tool(name = "add", description = "Add two numbers")]
async fn add(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a + b)))
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut agent = agent! {
        model: "qwen3-max",
        system_prompt: "You are a helpful math assistant",
        tools: [AddTool],
    }?;

    let answer = agent.execute("What is 1337 * 42?").await?;
    println!("{answer}");
    Ok(())
}
```

---

## Features

| Capability | Description |
|------------|-------------|
| 🔄 **ReAct Engine** | Thought → Action → Observation loop with Chain-of-Thought |
| 🔧 **Tool System** | `#[tool]` macro with auto JSON Schema; timeout + retry + parallelism |
| 🧠 **Dual-layer Memory** | `Store` (long-term KV) + `Checkpointer` (session history) |
| 📦 **Context Compression** | SlidingWindow / LLM Summary / Hybrid pipeline |
| 🤝 **Human-in-the-Loop** | Approval gates via Console, Webhook, or WebSocket |
| 🏗️ **Multi-Agent** | Orchestrator → SubAgent dispatch; Handoff between agents |
| 💡 **Skill System** | Tools + prompts packaged as reusable capability units |
| 🔌 **MCP Protocol** | Connect any MCP-compliant server (stdio / SSE / HTTP) |
| 📊 **Plan-and-Execute** | Planner + Executor: explicit plan → step execution strategy |
| 📡 **Streaming** | `execute_stream()` returns real-time `AgentEvent` stream |
| 📐 **Structured Output** | `extract::<T>()` — LLM output to typed Rust structs via JSON Schema |
| 🛡️ **Guard System** | Rule-based / LLM-powered content filtering on input & output |
| 🔑 **Permission Model** | Declarative tool permissions with pluggable policies |
| 📝 **Audit Logging** | Structured event logging with pluggable backends |
| 📈 **OpenTelemetry** | Distributed tracing and metrics via OTLP |
| 🎣 **Macro System** | `#[tool]`, `#[callback]`, `#[guard]`, `#[handler]`, `agent!{}`, `messages![]` |
| 🌐 **A2A Protocol** | Agent Card publishing and cross-framework collaboration |

---

## Workspace Structure

echo-agent is organized as a multi-crate workspace:

```
echo-agent/
├── echo-core/         Core traits & types (Tool, LlmClient, Agent, Guard, Error, etc.)
├── echo-macros/       Procedural macros (#[tool], #[callback], #[guard], #[handler], etc.)
├── echo-providers/    LLM provider implementations (OpenAI, Anthropic, Ollama)
├── echo-mcp/          MCP protocol client (stdio, SSE, HTTP transports)
├── src/               Main crate — agent engine, memory, skills, tools, and more
├── examples/          25 runnable demo programs
├── docs/              Bilingual documentation (en + zh)
└── skills/            External skill packs (Markdown-based)
```

End users depend only on the root `echo_agent` crate, which re-exports everything.

---

## Quick Start

### Prerequisites

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Configuration

Create `echo-agent.yaml` in your project root (or set `$ECHO_AGENT_CONFIG`):

```yaml
models:
  qwen3-max:
    base_url: https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions
    api_key: ${DASHSCOPE_API_KEY}

  gpt-4o:
    provider: openai
    api_key: ${OPENAI_API_KEY}
```

### Run an example

```bash
cargo run --example demo01_tools
cargo run --example demo25_macros
```

---

## Core Concepts

### 1. Tool — define with a single macro

```rust
use echo_agent::{tool, prelude::*};

#[tool(name = "weather", description = "Get weather for a city")]
async fn weather(
    /// City name
    city: String,
) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("Sunny in {city}")))
}

// Generates: WeatherParams + WeatherTool + impl TypedTool
```

### 2. Agent — build declaratively

```rust
use echo_agent::{agent, prelude::*};

let mut agent = agent! {
    model: "qwen3-max",
    system_prompt: "You are a helpful assistant",
    tools: [WeatherTool, CalculatorTool],
    max_iterations: 10,
}?;

let answer = agent.execute("What's the weather in Tokyo?").await?;
```

### 3. Callback — override only what you need

```rust
use echo_agent::{callback, prelude::*};

struct MyCallback;

#[callback]
impl MyCallback {
    async fn on_tool_start(&self, _agent: &str, tool: &str, _args: &serde_json::Value) {
        println!("[tool] {tool}");
    }
}
```

### 4. Guard — content filtering in one function

```rust
use echo_agent::{guard, prelude::*};

#[guard(name = "length-limit")]
async fn check_length(content: &str, direction: GuardDirection) -> Result<GuardResult> {
    if content.len() > 50000 {
        Ok(GuardResult::Block { reason: "Content too long".into() })
    } else {
        Ok(GuardResult::Pass)
    }
}
```

### 5. Streaming — real-time feedback

```rust
let mut stream = agent.execute_stream("Explain quantum entanglement").await?;
while let Some(event) = stream.next().await {
    match event? {
        AgentEvent::Token(t)           => print!("{t}"),
        AgentEvent::ToolCall { name, ..} => println!("\n[→ {name}]"),
        AgentEvent::FinalAnswer(a)     => { println!("\n{a}"); break; }
        _ => {}
    }
}
```

### 6. MCP — plug in any tool server

```rust
let mut mcp = McpManager::new();
let tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx", vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
)).await?;
agent.add_tools(tools);
```

---

## Macro Reference

| Macro | Type | Purpose |
|-------|------|---------|
| `#[tool]` | proc | Generate `TypedTool` from async fn |
| `#[callback]` | proc | Generate `AgentCallback` from impl block |
| `#[guard]` | proc | Generate `Guard` from async fn |
| `#[handler]` | proc | Generate `HumanLoopHandler` from impl block |
| `#[compressor]` | proc | Generate `ContextCompressor` from async fn |
| `#[permission_policy]` | proc | Generate `PermissionPolicy` from async fn |
| `#[audit_logger]` | proc | Generate `AuditLogger` from impl block |
| `agent!{}` | decl | Declarative agent construction |
| `messages![]` | decl | Quick message list builder |
| `tool_params!{}` | decl | JSON Schema builder for tool parameters |
| `chat_request!{}` | decl | Quick ChatRequest construction |

---

## Examples

| Example | What it demonstrates |
|---------|---------------------|
| [`demo01_tools`](examples/demo01_tools.rs) | Custom tools with `#[tool]` macro |
| [`demo02_tasks`](examples/demo02_tasks.rs) | DAG task planning |
| [`demo03_approval`](examples/demo03_approval.rs) | Human-in-the-loop approval |
| [`demo04_suagent`](examples/demo04_suagent.rs) | Orchestrator + SubAgent |
| [`demo05_compressor`](examples/demo05_compressor.rs) | Context compression |
| [`demo06_mcp`](examples/demo06_mcp.rs) | MCP tool server |
| [`demo07_skills`](examples/demo07_skills.rs) | Built-in skills |
| [`demo08_external_skills`](examples/demo08_external_skills.rs) | External skill loading |
| [`demo09_file_shell`](examples/demo09_file_shell.rs) | File and shell tools |
| [`demo10_streaming`](examples/demo10_streaming.rs) | Streaming output |
| [`demo11_callbacks`](examples/demo11_callbacks.rs) | Lifecycle callbacks |
| [`demo12_resilience`](examples/demo12_resilience.rs) | Retry, timeout, fault tolerance |
| [`demo13_tool_execution`](examples/demo13_tool_execution.rs) | Tool execution config |
| [`demo14_memory_isolation`](examples/demo14_memory_isolation.rs) | Memory isolation |
| [`demo15_structured_output`](examples/demo15_structured_output.rs) | JSON Schema output |
| [`demo16_testing`](examples/demo16_testing.rs) | Mock testing |
| [`demo17_chat`](examples/demo17_chat.rs) | Interactive chat |
| [`demo18_semantic_memory`](examples/demo18_semantic_memory.rs) | Semantic memory |
| [`demo19_guard`](examples/demo19_guard.rs) | Guard system |
| [`demo20_audit`](examples/demo20_audit.rs) | Audit logging |
| [`demo21_handoff`](examples/demo21_handoff.rs) | Agent handoff |
| [`demo22_plan_execute`](examples/demo22_plan_execute.rs) | Plan-and-Execute |
| [`demo23_a2a`](examples/demo23_a2a.rs) | A2A protocol |
| [`demo24_topology`](examples/demo24_topology.rs) | Topology visualization |
| [`demo25_macros`](examples/demo25_macros.rs) | Macro system showcase |

---

## Compatibility

Works with any **OpenAI-compatible** API, plus native Anthropic and Ollama support:

| Provider | Endpoint |
|----------|---------|
| OpenAI | `https://api.openai.com/v1` |
| Anthropic | `https://api.anthropic.com/v1` (native) |
| DeepSeek | `https://api.deepseek.com/v1` |
| Alibaba Qwen | `https://dashscope.aliyuncs.com/compatible-mode/v1` |
| Ollama (local) | `http://localhost:11434` (native) |
| LM Studio | `http://localhost:1234/v1` |

---

## Documentation

Full docs in [`docs/`](./docs/):

- [ReAct Agent](docs/en/01-react-agent.md)
- [Tool System](docs/en/02-tools.md)
- [Memory System](docs/en/03-memory.md)
- [Context Compression](docs/en/04-compression.md)
- [Human-in-the-Loop](docs/en/05-human-loop.md)
- [Multi-Agent](docs/en/06-subagent.md)
- [Skill System](docs/en/07-skills.md)
- [MCP Protocol](docs/en/08-mcp.md)
- [DAG Tasks](docs/en/09-tasks.md)
- [Streaming](docs/en/10-streaming.md)
- [Structured Output](docs/en/11-structured-output.md)
- [Mock Testing](docs/en/12-mock.md)

---

## Contributing

```bash
git clone https://github.com/your-org/echo-agent
cd echo-agent
cargo build
cargo test --lib
cargo run --example demo01_tools
```

Before submitting a PR:
- `cargo fmt` and `cargo clippy`
- Add tests for new functionality
- Update relevant docs in `docs/`

---

## License

MIT © echo-agent contributors
