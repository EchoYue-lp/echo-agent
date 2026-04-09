<div align="center">

# 🚀 echo-agent

**The Complete AI Agent Framework for Rust - Memory Safe, Async-Native, Production Ready**

[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-1.3.0-brightgreen)](https://github.com/EchoYue-lp/echo-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![OpenAI Compatible](https://img.shields.io/badge/API-OpenAI%20Compatible-green)](https://platform.openai.com/docs/api-reference)
[![Async](https://img.shields.io/badge/async-tokio-blue)](https://tokio.rs/)

**Zero-cost abstractions • Memory safe • Async-native • Production ready**

[中文文档](./README.zh.md) · [Documentation](./docs/en/README.md) · [Examples](./examples/)

</div>

---

## ✨ Why echo-agent?

Most AI agent frameworks are built in Python. **echo-agent** brings the full power of a modern Agent framework to Rust — matching feature parity with LangGraph, CrewAI, and AutoGen while delivering the **performance, reliability, and type safety** only Rust can offer.

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

## 🏗️ Architecture Overview

```
┌─────────────────────────────────────────────────────────┐
│                   User / Application                     │
└────────────────────────┬────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────┐
│                    ReactAgent                            │
│  ┌──────────────┐  ┌────────────┐  ┌─────────────────┐  │
│  │ContextManager│  │ToolManager │  │  SkillManager   │  │
│  │(compression) │  │(execution) │  │ (Skill metadata)│  │
│  └──────────────┘  └────────────┘  └─────────────────┘  │
│                                                         │
│  ┌──────────────┐  ┌────────────┐  ┌─────────────────┐  │
│  │  Checkpointer│  │   Store    │  │HumanApprovalMgr │  │
│  │(session hist)│  │(long-term) │  │ (approval gate) │  │
│  └──────────────┘  └────────────┘  └─────────────────┘  │
│                                                         │
│  ┌──────────────────────────────────────────────────┐   │
│  │            SubAgent Registry                      │   │
│  │  { "math_agent": Arc<AsyncMutex<Box<dyn Agent>>> │   │
│  │    "writer_agent": ... }                          │   │
│  └──────────────────────────────────────────────────┘   │
└────────────────────────┬────────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────┐
│                  LLM Provider                            │
│        (OpenAI / DeepSeek / Qwen / Ollama / ...)         │
└─────────────────────────────────────────────────────────┘
```

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

echo-agent is built around several key concepts that enable flexible, production-ready agent development:

### 1. ReAct Engine — Thought → Action → Observation loop
The foundation of echo-agent is the ReAct (Reasoning + Acting) pattern with built-in Chain-of-Thought prompting. Agents think step-by-step, decide which tool to call, observe results, and continue until they reach a final answer.

```rust
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("You are a helpful assistant")
    .build()?;
let answer = agent.execute("What is 42 * 1337?").await?;
```

### 2. Tool System — `#[tool]` macro + auto JSON Schema
Define tools as simple async functions. The `#[tool]` macro generates parameter schemas, descriptions, and the `TypedTool` implementation automatically.

```rust
use echo_agent::{tool, prelude::*};

#[tool(name = "weather", description = "Get weather for a city")]
async fn weather(city: String) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("Sunny in {city}")))
}

// Use it: agent.add_tool(Box::new(WeatherTool));
```

### 3. Dual-layer Memory — Store + Checkpointer
- **Store**: Long-term key-value storage with namespace isolation
- **Checkpointer**: Session history preservation across restarts

```rust
let store = Arc::new(InMemoryStore::new());
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .with_memory_tools(store)  // auto-injects remember/recall/search/forget
    .build()?;
```

### 4. Multi-Modal Messages — Text, images, files in one message
Send and receive images (base64 or URLs) and file attachments alongside text, compatible with OpenAI Vision and Anthropic APIs.

```rust
let msg = Message::user_with_image(
    "What's in this image?",
    "image/png",
    base64_data,
);
```

### 5. Context Compression — Sliding window, LLM summary, hybrid
Manage token limits with configurable compression strategies that preserve conversation context.

```rust
agent.set_compressor(Box::new(SlidingWindowCompressor::new(4096)));
```

### 6. Unified Retry Policy — One policy for all external calls
Configure retry, timeout, and backoff once, apply to LLM calls, MCP requests, A2A communication, and sandbox execution.

```rust
let policy = RetryPolicy::new(3, Duration::from_millis(500))
    .max_delay(Duration::from_secs(30))
    .jitter(true);
let response = with_retry(&policy, || llm_client.chat(request)).await?;
```

### 7. Dynamic Tool Management — Add/remove/replace tools mid-conversation
Adapt toolset based on conversation phase or user needs without restarting the agent.

```rust
agent.add_tool(Box::new(SearchWebTool));
agent.remove_tool("search_web");
agent.replace_tool(Box::new(SaferExecuteCodeTool));
```

### 8. Human-in-the-Loop — Approval gates for critical actions
Require human approval before executing sensitive tools via Console, Webhook, or WebSocket interfaces.

```rust
let approval = ConsoleApproval::new();
agent.set_human_loop_handler(Box::new(approval));
```

### 9. Multi-Agent Orchestration — Orchestrator + SubAgent teams
Coordinate multiple specialized agents with context isolation and handoff protocols.

```rust
let orchestrator = Orchestrator::new();
orchestrator.register("math", math_agent);
orchestrator.register("writer", writer_agent);
```

### 10. Skill System — Progressive capability disclosure
Packages of related tools and prompts that can be discovered, activated, and used on demand.

```rust
agent.load_skill("web_research").await?;  // loads SKILL.md + registers tools
```

### 11. MCP Protocol — Connect any Model Context Protocol server
Integrate filesystem, databases, browsers, and other resources via standardized MCP servers.

```rust
let mut mcp = McpManager::new();
let tools = mcp.connect(McpServerConfig::stdio(
    "filesystem", "npx", vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
)).await?;
agent.add_tools(tools);
```

### 12. Plan-and-Execute — Explicit planning phase before execution
Planner agent creates a task DAG, Executor agent follows it step-by-step with optional replanning.

```rust
let planner = PlanExecuteAgent::new(planner_config, executor_config);
let result = planner.execute("Research quantum computing trends").await?;
```

### 13. Streaming — Real-time token-by-token output
Receive `AgentEvent` streams including tokens, tool calls, and final answers as they happen.

```rust
let mut stream = agent.execute_stream("Explain quantum entanglement").await?;
while let Some(event) = stream.next().await {
    match event? {
        AgentEvent::Token(t) => print!("{t}"),
        AgentEvent::FinalAnswer(a) => { println!("\n{a}"); break; }
        _ => {}
    }
}
```

### 14. Structured Output — LLM responses to typed Rust structs
Extract structured data from LLM responses using JSON Schema validation.

```rust
#[derive(Serialize, Deserialize)]
struct Contact { name: String, email: String, phone: String }
let contacts: Vec<Contact> = agent.extract("Extract contacts from this text...").await?;
```

### 15. Declarative Workflow — YAML/JSON workflow definitions
Define agent graphs without writing Rust code.

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

### 16. Guard System — Rule-based and LLM-powered content filtering
Block or modify unsafe content on input and output with customizable guard pipelines.

```rust
#[guard(name = "length-limit")]
async fn check_length(content: &str, _: GuardDirection) -> Result<GuardResult> {
    if content.len() > 50000 {
        Ok(GuardResult::Block { reason: "Content too long".into() })
    } else {
        Ok(GuardResult::Pass)
    }
}
```

### 17. Graph Workflow Engine — LangGraph-style state machines
Build complex workflows with linear pipelines, conditional branches, loops, and parallel fan-out/fan-in.

```rust
let graph = GraphBuilder::new("etl_pipeline")
    .add_function_node("extract", |state| Box::pin(async move {
        state.set("data", vec!["hello", "world"]);
        Ok(())
    }))
    .add_edge("extract", "transform")
    .build()?;
```

### 18. IM Channels — Deploy agents to messaging platforms
Connect your agent to QQ (WebSocket) and Feishu (Webhook) with automatic token management and reconnection.

```rust
let mut manager = ChannelManager::new();
manager.register(Box::new(QqChannel::new(qq_config)?));
manager.register(Box::new(FeishuChannel::new(feishu_config)?));
manager.start_all(handler).await?;
```

### 19. Macro System — Declarative APIs for common patterns
`#[tool]`, `#[callback]`, `#[guard]`, `#[handler]`, `agent!{}`, `messages![]` and more.

```rust
#[callback]
impl MyCallback {
    async fn on_tool_start(&self, _agent: &str, tool: &str, _args: &serde_json::Value) {
        println!("[tool] {tool}");
    }
}
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
| [`demo26_provider_factory`](examples/demo26_provider_factory.rs) | Dynamic LLM provider factory |
| [`demo27_sqlite_memory`](examples/demo27_sqlite_memory.rs) | SQLite persistence with FTS5 + vector search |
| [`demo28_workflow`](examples/demo28_workflow.rs) | Workflow pipeline abstraction |
| [`demo29_sandbox`](examples/demo29_sandbox.rs) | Sandbox code execution |
| [`demo30_mcp_server`](examples/demo30_mcp_server.rs) | MCP server mode |
| [`demo31_memory_tools`](examples/demo31_memory_tools.rs) | **Memory tool auto-injection** |
| [`demo32_token_budget`](examples/demo32_token_budget.rs) | **Token budget control** |
| [`demo33_retry_policy`](examples/demo33_retry_policy.rs) | **Unified retry policy** |
| [`demo34_workflow_stream`](examples/demo34_workflow_stream.rs) | **Workflow streaming events** |
| [`demo35_dynamic_tools`](examples/demo35_dynamic_tools.rs) | **Dynamic tool registration** |
| [`demo36_multimodal`](examples/demo36_multimodal.rs) | **Multi-modal messages** |
| [`demo37_declarative_workflow`](examples/demo37_declarative_workflow.rs) | **Declarative YAML/JSON workflows** |
| [`demo38_im_channels`](examples/demo38_im_channels.rs) | **IM platform integration (QQ + Feishu)** |
| [`demo39_workflow`](examples/demo39_workflow.rs) | **Graph workflow engine with SharedState** |
| [`demo40_snapshot`](examples/demo40_snapshot.rs) | **Agent state snapshot and rollback** |

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
git clone https://github.com/EchoYue-lp/echo-agent
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
