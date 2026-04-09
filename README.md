<div align="center">

# echo-agent

**A Composable, Production-Ready AI Agent Framework for Rust**

[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-1.2.0-brightgreen)](https://github.com/your-org/echo-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![OpenAI Compatible](https://img.shields.io/badge/API-OpenAI%20Compatible-green)](https://platform.openai.com/docs/api-reference)
[![Async](https://img.shields.io/badge/async-tokio-blue)](https://tokio.rs/)

Build autonomous AI agents with Rust's **memory safety**, **zero-cost abstractions**, and **async-native concurrency**.

[中文文档](./README.zh.md) · [Documentation](./docs/en/README.md) · [Examples](./examples/)

</div>

---

## Why echo-agent?

Most AI agent frameworks are built in Python. **echo-agent** brings the full power of a modern Agent framework to Rust — matching feature parity with LangGraph, CrewAI, and AutoGen while delivering the performance, reliability, and type safety only Rust can offer.

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

**Go further** — deploy your agent on QQ, Feishu, or any IM platform with just a few lines:

```rust
let mut manager = ChannelManager::new();
manager.register(Box::new(QqChannel::new(qq_config)?));
manager.register(Box::new(FeishuChannel::new(feishu_config)?));
manager.start_all(handler).await?;  // done — your agent now lives in IM
```

---

## Highlights

- **40+ capabilities** — ReAct loop, tools, memory, streaming, multi-agent, skills, MCP, IM channels, guards, audit, and more
- **40 runnable examples** — every feature has a demo you can `cargo run` immediately
- **350+ unit tests** — comprehensive coverage across all modules
- **6 crates, 1 import** — modular workspace, but `use echo_agent::prelude::*` is all you need
- **Multi-modal** — text, images (base64 & URL), and file attachments in a single message
- **IM integration** — QQ Bot (WebSocket) & Feishu (Webhook) out of the box
- **Declarative workflows** — define agent graphs in YAML/JSON, no Rust code required
- **Unified retry** — one `RetryPolicy` for all external calls (LLM, MCP, A2A, sandbox)

---

## Feature Matrix

| Capability | Description |
|------------|-------------|
| 🔄 **ReAct Engine** | Thought → Action → Observation loop with Chain-of-Thought |
| 🔧 **Tool System** | `#[tool]` macro with auto JSON Schema; timeout + retry + parallelism |
| 🧠 **Dual-layer Memory** | `Store` (long-term KV) + `Checkpointer` (session history) |
| 🔍 **Memory Tools** | `with_memory_tools(store)` — auto-inject remember / recall / search / forget |
| 🖼️ **Multi-Modal** | `ContentPart::Text` / `ImageUrl` / `File` — images & files in messages |
| 📦 **Context Compression** | SlidingWindow / LLM Summary / Hybrid pipeline |
| 📏 **Token Budget** | `max_tool_output_tokens` auto-truncation + pre-think compression trigger |
| 🔁 **Unified Retry** | `RetryPolicy` + `with_retry` / `with_retry_if` for all external calls |
| 🔀 **Dynamic Tools** | `remove_tool()` / `replace_tool()` — adjust toolset mid-conversation |
| 🤝 **Human-in-the-Loop** | Approval gates via Console, Webhook, or WebSocket |
| 🏗️ **Multi-Agent** | Orchestrator → SubAgent dispatch; Handoff between agents |
| 💡 **Skill System** | Progressive disclosure: discover → activate → use |
| 🔌 **MCP Protocol** | Connect any MCP-compliant server (stdio / SSE / HTTP) |
| 📊 **Plan-and-Execute** | Planner + Executor: explicit plan → step execution |
| 📡 **Streaming** | `execute_stream()` returns real-time `AgentEvent` stream |
| 🌊 **Workflow Streaming** | `Graph::run_stream()` — per-node `WorkflowEvent` events |
| 📐 **Structured Output** | `extract::<T>()` — LLM output to typed Rust structs |
| 📝 **Declarative Workflow** | `Graph::from_yaml()` / `from_json()` — no-code workflow definition |
| 🛡️ **Guard System** | Rule-based / LLM-powered content filtering on input & output |
| 🔑 **Permission Model** | Declarative tool permissions with pluggable policies |
| 📋 **Audit Logging** | Structured event logging with pluggable backends |
| 📈 **OpenTelemetry** | Distributed tracing and metrics via OTLP |
| 🎣 **Macro System** | `#[tool]`, `#[callback]`, `#[guard]`, `#[handler]`, `agent!{}`, `messages![]` |
| 🌐 **A2A Protocol** | Agent Card publishing and cross-framework collaboration |
| 🏖️ **Sandbox** | Local / Docker / K8s code execution with resource limits |
| 🔗 **Graph Workflow** | Directed graph: linear, conditional, loop, parallel fan-out/fan-in |
| 💬 **IM Channels** | QQ Bot (WebSocket) & Feishu (Webhook) — plug your agent into IM |

---

## What's New in v1.2.0

### IM Channel Integration

Connect your Agent to real-world messaging platforms:

```rust
// QQ Bot — WebSocket gateway
let qq = QqChannel::new(QqConfig {
    app_id, client_secret,
})?;

// Feishu — HTTP webhook
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

Features:
- **Unified `ChannelPlugin` interface** — add new platforms by implementing one trait
- **Automatic token management** — OAuth caching and refresh, no manual handling
- **WebSocket reconnection** — exponential backoff, never drops silently
- **Message queuing** — async `mpsc` channel prevents lost messages under load
- **Whitelist support** — `ChatConfig::with_allow_from()` for access control

---

## v1.1.0

### Agent Directory Restructuring + StateGraph DSL

Agent modules reorganized: `src/agent/react_agent` → `src/agents/react`, `src/plan_execute` → `src/agents/plan_execute`. New `agents/self_reflection` module (Composite/Critic/LlmCritic). StateGraph DSL for declarative workflow definition. Enhanced Task system with DAG scheduling, Hooks, Events, Store persistence, and Executor.

### Human-in-the-Loop 7-Stage Pipeline

Full permission system inspired by Claude Code:

```
Bypass → Plan → Rules(deny-first) → ProtectedPaths → Cache(TTL) → DenialTracker → Mode dispatch
```

- **SessionApprovalCache** with configurable TTL (default 30 min)
- **Audit Trail**: `PermissionAuditSink` trait + InMemory/Logging/Composite implementations
- **ProtectedPathChecker**: `.git`/`.env`/`.ssh` always protected
- **AI Classifier**: RuleClassifier/LlmClassifier/CompositeClassifier for Auto mode
- **DenialTracker**: auto-fallback after consecutive denials
- **PermissionMode**: Default/Plan/Auto/AcceptEdits/BypassPermissions/DontAsk/Bubble

### Subagent System

Sync/Fork/Teammate three execution modes. SubagentBuilder/Registry/Executor/Hooks. Team collaboration with Coordinator + Mailbox.

---

## Previous: v1.0.0

### Memory Tool Auto-Injection

One line to give your agent persistent memory — no manual tool wiring:

```rust
let store = Arc::new(InMemoryStore::new());
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .with_memory_tools(store)  // registers remember + recall + search_memory + forget
    .build()?;
```

### Multi-Modal Support

Send images and files alongside text — compatible with OpenAI Vision and Anthropic APIs:

```rust
let msg = Message::user_with_image(
    "What's in this image?",
    "image/png",
    base64_data,
);
```

### Declarative Workflow (YAML/JSON)

Define agent graphs without writing Rust:

```yaml
name: research_pipeline
nodes:
  - name: researcher
    type: agent
    model: qwen3-max
    system_prompt: "You are a research assistant"
    input_key: task
    output_key: research
  - name: writer
    type: agent
    model: qwen3-max
    system_prompt: "You are a writing assistant"
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

### Unified Retry Policy

One retry strategy for all external calls:

```rust
let policy = RetryPolicy::new(3, Duration::from_millis(500))
    .max_delay(Duration::from_secs(30))
    .jitter(true);

let response = with_retry(&policy, || llm_client.chat(request)).await?;
```

### Dynamic Tool Management

Swap tools mid-conversation for multi-phase tasks:

```rust
agent.add_tool(Box::new(SearchWebTool));
agent.remove_tool("search_web");
agent.replace_tool(Box::new(SaferExecuteCodeTool));
```

---

## Workspace Structure

```
echo-agent/
├── echo-core/         Core traits & types (Tool, LlmClient, Agent, Guard, Retry, etc.)
├── echo-macros/       Procedural macros (#[tool], #[callback], #[guard], #[handler], etc.)
├── echo-providers/    LLM provider implementations (OpenAI, Anthropic, Ollama)
├── echo-mcp/          MCP protocol client (stdio, SSE, HTTP transports)
├── echo-channels/     IM channel plugins (QQ Bot, Feishu)
├── src/               Main crate — agent engine, memory, skills, tools, workflow, and more
├── examples/          40 runnable demo programs
├── docs/              Bilingual documentation (en + zh)
└── skills/            External skill packs (Markdown-based)

> **Note**: echo-agent is a pure library framework. For a ready-to-use Agent application with CLI and Web UI, see [echo-agent-cli](https://github.com/EchoYue-lp/echo-agent-cli).
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
cargo run --example demo34_workflow_stream
cargo run --example demo36_multimodal

# IM channels (requires env vars)
cargo run --example demo38_im_channels --features channels
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

### 3. Graph Workflow — orchestrate agent pipelines

```rust
let graph = GraphBuilder::new("etl_pipeline")
    .add_function_node("extract", |state| Box::pin(async move {
        state.set("data", vec!["hello", "world"]);
        Ok(())
    }))
    .add_function_node("transform", |state| Box::pin(async move {
        let data: Vec<String> = state.get("data").unwrap_or_default();
        state.set("result", data.iter().map(|s| s.to_uppercase()).collect::<Vec<_>>());
        Ok(())
    }))
    .set_entry("extract")
    .add_edge("extract", "transform")
    .set_finish("transform")
    .build()?;

let result = graph.run(SharedState::new()).await?;
```

### 4. Callback — override only what you need

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

### 5. Guard — content filtering in one function

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

### 6. Streaming — real-time feedback

```rust
let mut stream = agent.execute_stream("Explain quantum entanglement").await?;
while let Some(event) = stream.next().await {
    match event? {
        AgentEvent::Token(t)            => print!("{t}"),
        AgentEvent::ToolCall { name, .. } => println!("\n[→ {name}]"),
        AgentEvent::FinalAnswer(a)      => { println!("\n{a}"); break; }
        _ => {}
    }
}
```

### 7. MCP — plug in any tool server

```rust
let mut mcp = McpManager::new();
let tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx", vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
)).await?;
agent.add_tools(tools);
```

### 8. IM Channels — deploy to messaging platforms

```rust
let mut manager = ChannelManager::new();
manager.register(Box::new(QqChannel::new(qq_config)?));
manager.start_all(|_| Arc::new(MyHandler::new(llm))).await?;
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
| [`demo28_sandbox`](examples/demo28_sandbox.rs) | Sandbox code execution |
| [`demo29_workflow`](examples/demo29_workflow.rs) | Graph workflow engine |
| [`demo30_mcp_server`](examples/demo30_mcp_server.rs) | MCP server mode |
| [`demo31_memory_tools`](examples/demo31_memory_tools.rs) | **Memory tool auto-injection** |
| [`demo32_token_budget`](examples/demo32_token_budget.rs) | **Token budget control** |
| [`demo33_retry_policy`](examples/demo33_retry_policy.rs) | **Unified retry policy** |
| [`demo34_workflow_stream`](examples/demo34_workflow_stream.rs) | **Workflow streaming events** |
| [`demo35_dynamic_tools`](examples/demo35_dynamic_tools.rs) | **Dynamic tool registration** |
| [`demo36_multimodal`](examples/demo36_multimodal.rs) | **Multi-modal messages** |
| [`demo37_declarative_workflow`](examples/demo37_declarative_workflow.rs) | **Declarative YAML/JSON workflows** |
| [`demo38_im_channels`](examples/demo38_im_channels.rs) | **IM platform integration (QQ + Feishu)** |

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
- [IM Channels](docs/en/15-im-channels.md)

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
