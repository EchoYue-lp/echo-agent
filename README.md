<div align="center">

# echo-agent

### The Production-Grade AI Agent Framework for Rust

**ReAct Engine • Multi-Agent • Memory • Streaming • MCP • IM Channels • Workflows**

[![crates.io](https://img.shields.io/crates/v/echo_agent?color=brightgreen)](https://crates.io/crates/echo_agent)
[![docs.rs](https://docs.rs/echo_agent/badge.svg)](https://docs.rs/echo_agent)
[![CI](https://github.com/EchoYue-lp/echo-agent/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/EchoYue-lp/echo-agent/actions)
[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)
<a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="License: MIT"></a>
[![OpenAI Compatible](https://img.shields.io/badge/API-OpenAI%20Compatible-green)](https://platform.openai.com/docs/api-reference)
[![Async](https://img.shields.io/badge/runtime-tokio-blue)](https://tokio.rs/)

[中文文档](./README.zh.md) &middot; [Documentation](./docs/en/README.md) &middot; [Examples](./examples/) &middot; [Changelog](./CHANGELOG.md)

</div>

---

## Quick Start

Add to `Cargo.toml`:

```toml
[dependencies]
echo-agent = "0.2.0"
tokio = { version = "1", features = ["full"] }
```

Define a tool and run an agent — in under 20 lines:

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

## Why echo-agent?

Most AI agent frameworks live in Python. **echo-agent** brings full-featured Agent development to Rust — matching [LangGraph](https://github.com/langchain-ai/langgraph), [CrewAI](https://github.com/crewAIInc/crewAI), and [AutoGen](https://github.com/microsoft/autogen) feature parity, with the **performance, type safety, and reliability** that only Rust can deliver.

| | echo-agent | LangGraph | CrewAI | AutoGen |
|---|---|---|---|---|
| **Language** | Rust | Python | Python | Python |
| **Memory safety** | Compile-time | GC | GC | GC |
| **ReAct loop** | Built-in | Built-in | Built-in | Built-in |
| **Tool system** | `#[tool]` macro + JSON Schema | Decorator | Decorator | Function calling |
| **Multi-agent** | SubAgent + Handoff | Graph | Crew | Conversation |
| **Streaming** | Native async streams | Callback | Limited | Callback |
| **MCP protocol** | Native (stdio/SSE/HTTP) | Via LangChain | No | No |
| **IM channels** | QQ + Feishu built-in | No | No | No |
| **Workflow** | Graph + DAG + Sequential | StateGraph | Sequential | Sequential |
| **Context compression** | SlidingWindow + LLM + Hybrid | No | No | No |
| **Guardrails** | Rule + LLM filtering | No | No | No |
| **Sandbox** | Local + Docker + K8s | No | No | Docker |
| **Single binary deploy** | Yes | No | No | No |

### Deploy to IM in 5 lines

```rust,ignore
// Requires feature: channels
use echo_agent::channels::{ChannelManager, QqChannel, QqConfig, FeishuChannel, FeishuConfig};

let mut manager = ChannelManager::new();
manager.register(Box::new(QqChannel::new(QqConfig::new("app_id", "secret"))?));
manager.register(Box::new(FeishuChannel::new(FeishuConfig::new_long_poll("app_id".into(), "secret".into()))?));
manager.start_all(handler).await?;
```

### Run examples

```bash
cargo run --example demo01_tools          # Custom tools
cargo run --example demo25_macros         # Macro system
cargo run --example demo34_workflow_stream # Workflow streaming
cargo run --example demo36_multimodal     # Multi-modal messages
cargo run --example demo38_im_channels --features channels  # IM channels
```

### Feature Flags

The crate ships with **zero default features** (`default = []`) for minimal compile time and dependency footprint. Enable features individually or use `features = ["full"]` to get everything.

```toml
[dependencies]
echo-agent = { version = "0.2", features = ["full"] }
# or cherry-pick:
echo-agent = { version = "0.2", features = ["mcp", "sqlite", "web"] }
```

| Feature | In `full`? | Description |
|---------|-----------|-------------|
| `full` | — | Meta-feature: enables all flags listed below |
| `web` | yes | Web search + page fetch (DuckDuckGo/Brave/Tavily) |
| `mcp` | yes | Model Context Protocol client |
| `media` | yes | PDF/Excel/Word/image extraction |
| `channels` | yes | QQ Bot + Feishu IM integrations |
| `human-loop` | yes | Human-in-the-loop approval (Console/Webhook/WebSocket) |
| `plan-execute` | yes | Plan-then-execute agent pattern |
| `self-reflection` | yes | LLM self-critique and refinement |
| `subagent` | yes | Multi-agent orchestration |
| `sqlite` | yes | SQLite-backed persistent memory |
| `telemetry` | yes | OpenTelemetry tracing + metrics |
| `a2a` | yes | Agent-to-Agent protocol (server + client) |
| `handoff` | yes | Agent handoff/collaboration |
| `topology` | yes | Multi-agent topology tracking |
| `tasks` | yes | DAG task scheduling |
| `data` | yes | Polars-powered data tools |
| `rag` | yes | Retrieval-Augmented Generation |
| `research` | no | ArXiv, Semantic Scholar, PDF fetch, BibTeX tools |
| `chart` | yes | Chart generation tools |
| `git` | yes | Git operations tools |
| `shell` | yes | Restricted shell command execution |
| `files` | yes | File system read/write tools |
| `sandbox` | yes | Container/local sandbox execution |
| `database` | no | SQL database tools (requires `sqlx`) |
| `content-guard` | no | Content filtering guardrails |
| `project-rules` | no | `.claude/rules` project rule parsing |

---

## Architecture

```text
                              ┌─────────────┐
                              │   Your App   │
                              └──────┬───────┘
                                     │
                    ┌────────────────▼────────────────┐
                    │          ReactAgent              │
                    │                                  │
                    │  ┌──────────┐  ┌──────────────┐  │
                    │  │  Context  │  │    Tools      │  │
                    │  │ Manager   │  │   Manager     │  │
                    │  │(compress) │  │(retry/limit)  │  │
                    │  └──────────┘  └──────────────┘  │
                    │                                  │
                    │  ┌──────────┐  ┌──────────────┐  │
                    │  │  Memory   │  │    Human      │  │
                    │  │Store+Cp   │  │ Approval      │  │
                    │  └──────────┘  └──────────────┘  │
                    │                                  │
                    │  ┌──────────┐  ┌──────────────┐  │
                    │  │  Skills   │  │   SubAgent    │  │
                    │  │ Registry  │  │   Registry    │  │
                    │  └──────────┘  └──────────────┘  │
                    └────────────────┬────────────────┘
                                     │
              ┌──────────────────────▼──────────────────────┐
              │              LLM Providers                    │
              │  OpenAI · Anthropic · DeepSeek · Qwen · Ollama │
              └─────────────────────────────────────────────┘
```

---

## Feature Matrix

echo-agent ships with **67 registered tools** across 6 crates, all accessible through a single `use echo_agent::prelude::*`.

### Core

| Feature | Description | API Preview |
|---------|-------------|-------------|
| **ReAct Engine** | Thought → Action → Observation loop with CoT | `agent.execute("task").await?` |
| **Tool System** | `#[tool]` macro with auto JSON Schema, timeout + retry | `#[tool(name = "calc")] async fn calc(...)` |
| **Dual-layer Memory** | `Store` (long-term KV) + `Checkpointer` (session) | `.with_memory_tools(store)` |
| **Context Compression** | SlidingWindow / LLM Summary / Hybrid | `SlidingWindowCompressor::new(4096)` |
| **Token Budget** | Auto-truncation + pre-think compression trigger | `.max_tool_output_tokens(2000)` |
| **Unified Retry** | One `RetryPolicy` for LLM, MCP, A2A, sandbox | `with_retry(&policy, \|\| ...)` |
| **Dynamic Tools** | Add / remove / replace tools mid-conversation | `agent.remove_tool("old")` |
| **Streaming** | Real-time `AgentEvent` stream (tokens + tool calls) | `agent.execute_stream(task).await?` |
| **Structured Output** | LLM output → typed Rust structs via JSON Schema | `agent.extract::<Contact>(text)` |
| **Multi-Modal** | Text + images (base64/URL) + files in one message | `Message::user_with_image(...)` |
| **Guard System** | Rule-based / LLM-powered content filtering | `#[guard(name = "safety")] async fn ...` |
| **Permission Model** | Declarative tool permissions with pluggable policies | `DefaultPermissionPolicy::new()` |
| **Audit Logging** | Structured events with pluggable backends | `agent.set_audit_logger(...)` |
| **Macro System** | 11 macros: `#[tool]`, `agent!{}`, `messages![]`, ... | `agent! { model: "..", tools: [...] }` |

### Multi-Agent & Orchestration

| Feature | Description | API Preview |
|---------|-------------|-------------|
| **SubAgent** | Sync / Fork / Teammate execution modes | `agent.register_agent(sub)` |
| **Agent Handoff** | Context-aware transfer between agents | `HandoffManager::new()` |
| **Plan-and-Execute** | Explicit planning phase → step-by-step execution | `PlanExecuteAgent::new(...)` |
| **Self-Reflection** | LLM-based self-critique and refinement loops | `SelfReflectionAgent::new(...)` |
| **Graph Workflow** | Linear, conditional, loop, parallel fan-out/fan-in | `GraphBuilder::new("pipeline")` |
| **DAG Tasks** | Dependency-aware task scheduling with hooks | `TaskManager::default()` |
| **Declarative Workflow** | Define graphs in YAML/JSON — no Rust code needed | `Graph::from_yaml("wf.yaml")?` |

### Integrations

| Feature | Description | API Preview |
|---------|-------------|-------------|
| **MCP Protocol** | Connect any MCP server (stdio / SSE / HTTP) | `mcp.connect(McpServerConfig::stdio(...))` |
| **A2A Protocol** | Agent Card publishing, cross-framework collaboration | `A2AServer::bind("0.0.0.0:3000")` |
| **Skill System** | Progressive disclosure: discover → activate → use | `agent.load_skill("web_research")` |
| **IM Channels** | QQ Bot (WebSocket) & Feishu (Webhook) built-in | `ChannelManager::new()` |
| **Web Tools** | Search (DuckDuckGo/Brave/Tavily) + Page Fetch | `WebSearchTool::auto()` |
| **Research Tools** | ArXiv, Semantic Scholar, PDF fetch, BibTeX generation | `ArxivSearchTool` |
| **Media Tools** | PDF, Excel, Word, Image analysis built-in | `ImageAnalysisTool` |
| **Data Tools** | Polars-powered filter, aggregate, transform, stats | `DataReadTool` |
| **Sandbox** | Local / Docker / K8s code execution with limits | `LocalSandbox::new()` |
| **OpenTelemetry** | Distributed tracing and metrics via OTLP | `init_telemetry(&config)` |
| **Snapshot/Rollback** | Capture & restore agent state at any point | `agent.snapshot()` / `agent.rollback(1)` |
| **Circuit Breaker** | Auto-fail-fast when LLM is down | `agent.set_circuit_breaker(config)` |

---

## Feature Flags

```toml
# Minimal — just the ReAct engine
echo-agent = { version = "0.1.4", default-features = false }

# Full (default) — all features enabled
echo-agent = "0.2.0"

# Pick only what you need
echo-agent = { version = "0.1.4", default-features = false, features = ["mcp", "web"] }
```

| Feature | Enables | Key Dependencies |
|---------|---------|------------------|
| `mcp` | MCP protocol client | `echo-mcp`, `tokio-tungstenite` |
| `web` | Web search + fetch tools | `scraper`, `html2text` |
| `media` | PDF, Excel, Word, Image tools | `lopdf`, `calamine`, `docx-rs` |
| `data` | Polars data analysis | `polars` |
| `sqlite` | SQLite memory persistence | `rusqlite` |
| `channels` | QQ Bot + Feishu integrations | `echo-channels` |
| `human-loop` | Human-in-the-loop approvals | `tokio-tungstenite` |
| `tasks` | DAG task management | — |
| `workflow` | Graph workflow engine | — |
| `plan-execute` | Plan-and-Execute agent | — |
| `self-reflection` | Self-critique agent | — |
| `subagent` | Multi-agent orchestration | — |
| `handoff` | Agent handoff | — |
| `a2a` | Agent-to-Agent protocol | — |
| `topology` | Agent topology visualization | — |
| `telemetry` | OpenTelemetry tracing | `opentelemetry` |
| `sandbox` | Code execution sandbox (Local/Docker/K8s) | — |
| `semantic-memory` | Semantic memory | — |
| `macros` | Procedural macros (#\[tool\] etc.) | `echo-macros` |
| `provider-factory` | LLM provider factory | — |
| `multimodal` | Multimodal input (images/files) | — |
| `git` | Git operation tools | — |
| `database` | Database query tools | `sqlx` |
| `rag` | RAG retrieval tools | — |
| `research` | ArXiv, Semantic Scholar, PDF fetch, BibTeX | `quick-xml` |
| `chart` | Chart generation tools | — |
| `content-guard` | Content safety guard | — |
| `project-rules` | Project rule loading | — |
| `shell` | Shell command execution | — |
| `files` | File read/write tools | — |

---

## Workspace Structure

```text
echo-agent/
├── echo-core/           Core traits: Tool, Agent, LlmClient, Guard, Error, Retry
├── echo-macros/         Procedural macros: #[tool], #[callback], #[guard], #[handler]
├── echo-execution/      Sandbox, skills, and tool execution
├── echo-state/          Memory, compression, and audit logging
├── echo-orchestration/  Workflow, human-loop, and DAG tasks
├── echo-integration/    LLM providers, MCP, and IM channels (QQ/Feishu)
├── src/                 Agent engine, re-exports, and facade layer
├── examples/            33 runnable demos
├── docs/                Bilingual documentation (en + zh)
├── skills/              External skill packs (Markdown-based)
└── echo-agent.yaml      Example configuration
```

> **Note:** `echo-agent` is a library framework. For a ready-to-use application with CLI, Web UI, and WebSocket, see [echo-agent-cli](https://github.com/EchoYue-lp/echo-agent-cli).

---

## Configuration

Create `echo-agent.yaml` in your project root:

```yaml
# Provider / model registry (used by ProviderFactory and config-backed clients)
models:
  qwen3-max:
    provider: dashscope
    api_key: ${DASHSCOPE_API_KEY}

  deepseek-chat:
    provider: deepseek
    api_key: ${DEEPSEEK_API_KEY}

# Embedding config (used by semantic memory / vector search demos)
embedding:
  base_url: https://api.openai.com
  api_key: ${OPENAI_API_KEY}
  model: text-embedding-3-small
  timeout_secs: 30

# Runtime app config (used by examples such as IM channels)
model:
  name: qwen3-max
  max_tokens: 4096
  temperature: 0.7

agent:
  name: my-assistant
  system_prompt: "You are a helpful assistant."
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

Notes:

- `models:` is the registry used by `ProviderFactory`, `LlmConfig::from_model()`, and config-backed LLM clients.
- `embedding:` is used by semantic memory / vector search examples.
- `model:` / `agent:` / `channels:` / `mcp:` / `server:` / `logging:` are the framework runtime settings loaded by `echo_agent::config`.

Set secrets via environment variables:

```bash
export DASHSCOPE_API_KEY=sk-xxx      # Alibaba Qwen
export DEEPSEEK_API_KEY=sk-xxx       # DeepSeek
export OPENAI_API_KEY=sk-xxx         # OpenAI
export ANTHROPIC_API_KEY=sk-ant-xxx  # Anthropic
export QQ_APP_ID=your-qq-app-id
export QQ_CLIENT_SECRET=your-qq-client-secret
export FEISHU_APP_ID=your-feishu-app-id
export FEISHU_APP_SECRET=your-feishu-app-secret
```

---

## Highlights

- **67 registered tools** — ReAct loop, data analysis, research papers, web, media, RAG, database, and more
- **33 runnable examples** — every feature has a demo you can `cargo run` immediately
- **Comprehensive unit tests** — full coverage across all modules
- **6 crates, 1 import** — modular workspace, but `use echo_agent::prelude::*` is all you need
- **Multi-modal** — text, images (base64 & URL), and file attachments in a single message
- **IM integration** — QQ Bot (WebSocket) & Feishu (Webhook) out of the box
- **Declarative workflows** — define agent graphs in YAML/JSON, no Rust code required
- **Unified retry** — one `RetryPolicy` for all external calls (LLM, MCP, A2A, sandbox)
- **Zero-cost abstractions** — compiled to native code, no runtime overhead

---

## Core Concepts

echo-agent is built around several key concepts that enable flexible, production-ready agent development:

### 1. ReAct Engine — Thought → Action → Observation loop

The foundation of echo-agent is the ReAct (Reasoning + Acting) pattern with built-in Chain-of-Thought prompting. Agents think step-by-step, decide which tool to call, observe results, and continue until they reach a final answer.

```rust,no_run
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .system_prompt("You are a helpful assistant")
        .build()?;
    let answer = agent.execute("What is 42 * 1337?").await?;
    println!("{answer}");
    Ok(())
}
```

Three builder presets for different needs:

```rust,no_run
use echo_agent::prelude::*;

fn main() -> echo_agent::error::Result<()> {
    // Minimal — no tools, no memory, just chat
    let _agent = ReactAgentBuilder::simple("qwen3-max", "Be helpful")?;

    // Standard — tools + CoT enabled
    let _agent = ReactAgentBuilder::standard("qwen3-max", "assistant", "Be helpful")?;

    // Full-featured — tools + memory + tasks + CoT
    let _agent = ReactAgentBuilder::full_featured("qwen3-max", "assistant", "Be helpful")?;
    Ok(())
}
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

Built-in media tools (feature `media`): PDF extract/info, Excel read/info/to_csv, Word read/info/structure, Image analysis, Text read/search/stats/process/export.

Built-in data tools (feature `data`): Polars-powered read/filter/aggregate/stats/transform/export.

### 3. Dual-layer Memory — Store + Checkpointer

- **Store**: Long-term key-value storage with namespace isolation (`InMemoryStore`, `FileStore`, `SqliteStore`)
- **Checkpointer**: Session history preservation across restarts (`FileCheckpointer`, `InMemoryCheckpointer`)

One line to give your agent persistent memory — no manual tool wiring:

```rust,no_run
use echo_agent::prelude::*;
use std::sync::Arc;

fn main() -> echo_agent::error::Result<()> {
    let store = Arc::new(InMemoryStore::new());
    let _agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .with_memory_tools(store)  // registers remember + recall + search_memory + forget
        .build()?;
    Ok(())
}
```

### 4. Multi-Modal Messages — Text, images, files in one message

Send and receive images (base64 or URLs) and file attachments alongside text, compatible with OpenAI Vision and Anthropic APIs.

```rust
use echo_agent::prelude::*;

fn main() {
    let base64_data = "...";  // your base64-encoded image
    let _msg = Message::user_with_image(
        "What's in this image?",
        "image/png",
        base64_data,
    );
}
```

### 5. Context Compression — Sliding window, LLM summary, hybrid

Manage token limits with configurable compression strategies that preserve conversation context.

```rust,no_run
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .build()?;
    agent.set_compressor(SlidingWindowCompressor::new(4096)).await;
    Ok(())
}
```

Three strategies:
- **SlidingWindow** — keeps the most recent messages within token budget
- **SummaryCompressor** — uses LLM to summarize older messages
- **HybridCompressor** — combines both for best quality

**Token counting** — estimate token usage before calling the LLM:

```rust
use echo_agent::prelude::*;

fn main() {
    let tokenizer = HeuristicTokenizer;
    let count = tokenizer.count_tokens("Hello, world!");
    println!("~{count} tokens");  // ~4 tokens

    // For cost tracking across requests:
    use echo_agent::tokenizer::TokenUsageTracker;
    let tracker = TokenUsageTracker::new("gpt-4o");
    tracker.record(1500, 800, Some(2300));
    println!("{}", tracker.summary());
}
```

### 6. Unified Retry Policy — One policy for all external calls

Configure retry, timeout, and backoff once, apply to LLM calls, MCP requests, A2A communication, and sandbox execution.

```rust
use echo_agent::prelude::*;
use std::time::Duration;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let policy = RetryPolicy::new(3, Duration::from_millis(500))
        .max_delay(Duration::from_secs(30))
        .jitter(true);
    // Apply to any fallible async operation:
    let _response = with_retry(&policy, || async { Ok::<_, &str>("done") }).await.unwrap();
    Ok(())
}
```

### 7. Dynamic Tool Management — Add/remove/replace tools mid-conversation

Adapt toolset based on conversation phase or user needs without restarting the agent.

```rust,no_run
use echo_agent::{prelude::*, tool};

// User-defined tools (via #[tool] macro):
#[tool(name = "search_web", description = "Search the web")]
async fn search_web(query: String) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("Results for: {query}")))
}

fn main() -> echo_agent::error::Result<()> {
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .build()?;
    agent.add_tool(Box::new(SearchWebTool));
    agent.remove_tool("search_web");
    agent.replace_tool(Box::new(SearchWebTool));
    Ok(())
}
```

### 8. Human-in-the-Loop — Approval gates for critical actions

Require human approval before executing sensitive tools via Console, Webhook, or WebSocket interfaces.

```rust,ignore
// Requires feature: human-loop
use echo_agent::prelude::*;
use echo_agent::advanced::ConsoleHumanLoopProvider;
use std::sync::Arc;

fn main() -> echo_agent::error::Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .build()?;
    let approval: Arc<ConsoleHumanLoopProvider> = Arc::new(ConsoleHumanLoopProvider);
    agent.set_human_loop_provider(approval);
    Ok(())
}
```

Full 7-stage permission pipeline (inspired by Claude Code):

```text
Bypass → Plan → Rules(deny-first) → ProtectedPaths → Cache(TTL) → DenialTracker → Mode dispatch
```

- **SessionApprovalCache** with configurable TTL (default 30 min)
- **Audit Trail**: `PermissionAuditSink` trait + InMemory/Logging/Composite implementations
- **ProtectedPathChecker**: `.git`/`.env`/`.ssh` always protected
- **AI Classifier**: RuleClassifier/LlmClassifier/CompositeClassifier for Auto mode
- **DenialTracker**: auto-fallback after consecutive denials
- **PermissionMode**: Default/Plan/Auto/AcceptEdits/BypassPermissions/DontAsk/Bubble

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() {
    // Grant read access, require approval for execute
    let policy = DefaultPermissionPolicy::new()
        .grant(ToolPermission::Read)
        .require_approval(ToolPermission::Execute);

    let decision = policy.check("shell", &[ToolPermission::Execute]).await;
    assert!(decision.requires_approval());
}
```

### 9. Multi-Agent Orchestration — Orchestrator + SubAgent teams

Coordinate multiple specialized agents with context isolation and handoff protocols.

Three execution modes:
- **Sync** — parent blocks until subagent returns
- **Fork** — subagent runs in background, parent continues
- **Teammate** — collaborative mode with shared Mailbox

```rust,ignore
// Requires feature: subagent
use echo_agent::prelude::*;

fn main() -> echo_agent::error::Result<()> {
    let math_agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("math_expert")
        .system_prompt("You solve math problems.")
        .build()?;

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .enable_subagent()
        .build()?;

    agent.register_agent(Box::new(math_agent));
    Ok(())
}
```

### 10. Skill System — Progressive capability disclosure

Packages of related tools and prompts that can be discovered, activated, and used on demand.

```rust,ignore
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .build()?;

    // Discover and activate file-based skills (SKILL.md packs):
    agent.load_skills_from_dir("./skills/web_research").await?;
    Ok(())
}
```

Pre-built skills: `code_review`, `data_analyst`, `project-stats`, `python-linter`, `web_researcher`.

### 11. MCP Protocol — Connect any Model Context Protocol server

Integrate filesystem, databases, browsers, and other resources via standardized MCP servers.

```rust,ignore
// Requires feature: mcp
use echo_agent::prelude::*;
use echo_agent::advanced::{McpManager, McpServerConfig};

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let mut mcp = McpManager::new();
    let tools = mcp.connect(McpServerConfig::stdio(
        "filesystem", "npx", vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
    )).await?;

    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .build()?;
    agent.add_tools(tools);
    Ok(())
}
```

Supports three transports: **stdio**, **SSE**, **HTTP**.

### 12. Plan-and-Execute — Explicit planning phase before execution

Planner agent creates a task DAG, Executor agent follows it step-by-step with optional replanning.

```rust,ignore
// Requires feature: plan-execute
use echo_agent::prelude::*;
use echo_agent::advanced::PlanExecuteAgent;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let planner = PlanExecuteAgent::new("research_agent", planner_config, executor_config);
    let result = planner.execute("Research quantum computing trends").await?;
    println!("{result}");
    Ok(())
}
```

### 13. Streaming — Real-time token-by-token output

Receive `AgentEvent` streams including tokens, tool calls, and final answers as they happen.

```rust,no_run
use echo_agent::prelude::*;
use futures::StreamExt;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .build()?;
    let mut stream = agent.execute_stream("Explain quantum entanglement").await?;
    while let Some(event) = stream.next().await {
        match event? {
            AgentEvent::Token(t) => print!("{t}"),
            AgentEvent::FinalAnswer(a) => { println!("\n{a}"); break; }
            _ => {}
        }
    }
    Ok(())
}
```

### 14. Structured Output — LLM responses to typed Rust structs

Extract structured data from LLM responses using JSON Schema validation.

```rust,no_run
use echo_agent::prelude::*;
use echo_agent::llm::ResponseFormat;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, Debug)]
struct Contact { name: String, email: String, phone: String }

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .system_prompt("You are an extraction assistant")
        .build()?;
    let contacts: Vec<Contact> = agent.extract(
        "Extract contacts from this text...",
        ResponseFormat::json_schema("contacts", json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "email": {"type": "string"},
                    "phone": {"type": "string"}
                },
                "required": ["name", "email", "phone"]
            }
        })),
    ).await?;
    println!("{:?}", contacts);
    Ok(())
}
```

### 15. Declarative Workflow — YAML/JSON workflow definitions

Define agent graphs without writing Rust code.

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

```rust,no_run
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let wf = WorkflowDefinition::from_yaml("workflow.yaml")?;
    let graph = wf.build_graph()?;
    let state = SharedState::new();
    let result = graph.run(state).await?;
    Ok(())
}
```

### 16. Guard System — Rule-based and LLM-powered content filtering

Block or modify unsafe content on input and output with customizable guard pipelines.

```rust
use echo_agent::{guard, prelude::*};

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

```rust,no_run
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let state = SharedState::new();
    let graph = GraphBuilder::new("etl_pipeline")
        .add_function_node("extract", |state| Box::pin(async move {
            state.set("data", vec!["hello", "world"])?;
            Ok(())
        }))
        .add_function_node("transform", |state| Box::pin(async move {
            // transform data...
            Ok(())
        }))
        .add_edge("extract", "transform")
        .add_edge("transform", Graph::END)
        .build()?;
    
    let result = graph.run(state).await?;
    Ok(())
}
```

Also supports **streaming execution**: `graph.run_stream(state).await?` yields `WorkflowEvent` per node.

### 18. IM Channels — Deploy agents to messaging platforms

Connect your agent to QQ (WebSocket) and Feishu (Webhook) with automatic token management and reconnection.

```rust,ignore
// Requires feature: channels
use echo_agent::channels::{ChannelManager, QqChannel, QqConfig, FeishuChannel, FeishuConfig};

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    // QQ Bot — WebSocket gateway
    let qq = QqChannel::new(QqConfig::new("your_app_id", "your_client_secret"))?;

    // Feishu — HTTP webhook
    let feishu = FeishuChannel::new(FeishuConfig::new_webhook(
        "your_app_id".into(),
        "your_app_secret".into(),
        "0.0.0.0:8080".into(),
        "/webhook".into(),
        None,
    ))?;

    let mut manager = ChannelManager::new();
    manager.register(Box::new(qq));
    manager.register(Box::new(feishu));
    manager.start_all(handler).await?;
    Ok(())
}
```

Features:
- **Unified `ChannelPlugin` interface** — add new platforms by implementing one trait
- **Automatic token management** — OAuth caching and refresh, no manual handling
- **WebSocket reconnection** — exponential backoff, never drops silently
- **Message queuing** — async `mpsc` channel prevents lost messages under load
- **Whitelist support** — `ChatConfig::with_allow_from()` for access control

### 19. Macro System — Declarative APIs for common patterns

`#[tool]`, `#[callback]`, `#[guard]`, `#[handler]`, `agent!{}`, `messages![]` and more.

```rust
use echo_agent::callback;

struct MyCallback;

#[callback]
impl MyCallback {
    async fn on_tool_start(&self, _agent: &str, tool: &str, _args: &serde_json::Value) {
        println!("[tool] {tool}");
    }
}
```

### 20. Web Tools — Search the internet and fetch web pages

Give your Agent real-time internet access with web search and page fetching.

```rust,ignore
// Requires feature: web
use echo_agent::prelude::*;
use echo_agent::tools::web::{WebSearchTool, WebFetchTool};

fn main() -> echo_agent::error::Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .build()?;

    // Auto-select best provider: Tavily > Brave > DuckDuckGo
    agent.add_tool(Box::new(WebSearchTool::auto()));
    agent.add_tool(Box::new(WebFetchTool::new()));
    Ok(())
}
```

| Provider | Cost | Quality | Notes |
|----------|------|---------|-------|
| DuckDuckGo | Free | Medium | HTML scraping, no API key needed |
| Brave | Free 2k/mo | High | Official API |
| Tavily | Paid (free tier) | Highest | AI-optimized for agents |

### 21. Self-Reflection Agent — LLM self-critique and refinement

```rust,ignore
// Requires feature: self-reflection
use echo_agent::prelude::*;
use echo_agent::advanced::LlmCritic;
use echo_agent::agent::self_reflection::SelfReflectionAgent;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let generator = ReactAgentBuilder::new()
        .model("qwen3-max")
        .system_prompt("You are a technical writer.")
        .build()?;

    let critic = LlmCritic::new("qwen3-max");

    let agent = SelfReflectionAgent::new("reflection_agent", generator, critic)
        .max_reflections(3);

    let result = agent.execute("Write a summary of quantum computing").await?;
    println!("{result}");
    Ok(())
}
```

### 22. Snapshot & Rollback — Time-travel debugging

```rust,no_run
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .snapshot_policy(SnapshotPolicy::default())
        .build()?;
    let snapshot_id = agent.snapshot().await;  // Option<String>
    // ... some operations that go wrong ...
    if let Some(id) = snapshot_id {
        agent.rollback_to(&id).await;          // rollback to specific snapshot
    }
    agent.rollback(1).await;                   // go back 1 step
    Ok(())
}
```

### 23. Circuit Breaker — Auto-fail-fast when LLM is down

```rust,no_run
use echo_agent::prelude::*;
use std::time::Duration;

fn main() -> echo_agent::error::Result<()> {
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .build()?;
    let cb_config = CircuitBreakerConfig {
        failure_threshold: 5,
        timeout: Duration::from_secs(30),
        ..Default::default()
    };
    agent.set_circuit_breaker(cb_config);
    Ok(())
}
```

---

## Macro Reference

| Macro | Type | Generates |
|-------|------|-----------|
| `#[tool]` | Proc | `TypedTool` from async fn |
| `#[callback]` | Proc | `AgentCallback` from impl block |
| `#[guard]` | Proc | `Guard` from async fn |
| `#[handler]` | Proc | `HumanLoopHandler` from impl block |
| `#[compressor]` | Proc | `ContextCompressor` from async fn |
| `#[permission_policy]` | Proc | `PermissionPolicy` from async fn |
| `#[audit_logger]` | Proc | `AuditLogger` from impl block |
| `agent!{}` | Decl | Agent construction |
| `messages![]` | Decl | Message list builder |
| `tool_params!{}` | Decl | JSON Schema builder |
| `chat_request!{}` | Decl | ChatRequest construction |

---

## Examples

Examples are classified into `Acceptance`, `Conditional acceptance`, and `Teaching` contracts.
See `examples/README.md` for the full bucketed inventory and maintenance rules.

| # | Example | Demonstrates |
|---|---------|-------------|
| 01 | [`demo01_tools`](examples/demo01_tools.rs) | Custom tools with `#[tool]` |
| 02 | [`demo02_tasks`](examples/demo02_tasks.rs) | DAG task planning |
| 03 | [`demo03_approval`](examples/demo03_approval.rs) | Human-in-the-loop |
| 04 | [`demo04_subagent`](examples/demo04_subagent.rs) | Multi-agent orchestration |
| 05 | [`demo05_compressor`](examples/demo05_compressor.rs) | Context compression |
| 06 | [`demo06_mcp`](examples/demo06_mcp.rs) | MCP tool server |
| 07 | [`demo07_skills`](examples/demo07_skills.rs) | Built-in skills |
| 08 | [`demo08_external_skills`](examples/demo08_external_skills.rs) | External skill loading |
| 09 | [`demo09_file_shell`](examples/demo09_file_shell.rs) | File & shell tools |
| 10 | [`demo10_streaming`](examples/demo10_streaming.rs) | Streaming output |
| 11 | [`demo11_callbacks`](examples/demo11_callbacks.rs) | Lifecycle callbacks |
| 12 | [`demo12_resilience`](examples/demo12_resilience.rs) | Retry & fault tolerance |
| 13 | [`demo13_tool_execution`](examples/demo13_tool_execution.rs) | Tool execution config |
| 14 | [`demo14_memory_isolation`](examples/demo14_memory_isolation.rs) | Memory isolation |
| 15 | [`demo15_structured_output`](examples/demo15_structured_output.rs) | JSON Schema output |
| 16 | [`demo16_testing`](examples/demo16_testing.rs) | Mock testing |
| 17 | [`demo17_chat`](examples/demo17_chat.rs) | Interactive chat |
| 18 | [`demo18_semantic_memory`](examples/demo18_semantic_memory.rs) | Semantic memory |
| 19 | [`demo19_guard`](examples/demo19_guard.rs) | Guard system |
| 20 | [`demo20_audit`](examples/demo20_audit.rs) | Audit logging |
| 21 | [`demo21_handoff`](examples/demo21_handoff.rs) | Agent handoff |
| 22 | [`demo22_plan_execute`](examples/demo22_plan_execute.rs) | Plan-and-Execute |
| 23 | [`demo23_a2a`](examples/demo23_a2a.rs) | A2A protocol |
| 24 | [`demo24_topology`](examples/demo24_topology.rs) | Topology visualization |
| 25 | [`demo25_macros`](examples/demo25_macros.rs) | Macro system showcase |
| 26 | [`demo26_provider_factory`](examples/demo26_provider_factory.rs) | Dynamic LLM factory |
| 27 | [`demo27_sqlite_memory`](examples/demo27_sqlite_memory.rs) | SQLite persistence |
| 28 | [`demo28_workflow`](examples/demo28_workflow.rs) | Workflow pipeline |
| 29 | [`demo29_sandbox`](examples/demo29_sandbox.rs) | Sandbox execution |
| 30 | [`demo30_mcp_server`](examples/demo30_mcp_server.rs) | MCP server mode |
| 31 | [`demo31_memory_tools`](examples/demo31_memory_tools.rs) | Memory tool injection |
| 32 | [`demo32_token_budget`](examples/demo32_token_budget.rs) | Token budget control |
| 33 | [`demo33_retry_policy`](examples/demo33_retry_policy.rs) | Unified retry |
| 34 | [`demo34_workflow_stream`](examples/demo34_workflow_stream.rs) | Workflow streaming |
| 35 | [`demo35_dynamic_tools`](examples/demo35_dynamic_tools.rs) | Dynamic tool management |
| 36 | [`demo36_multimodal`](examples/demo36_multimodal.rs) | Multi-modal messages |
| 37 | [`demo37_declarative_workflow`](examples/demo37_declarative_workflow.rs) | YAML/JSON workflows |
| 38 | [`demo38_im_channels`](examples/demo38_im_channels.rs) | IM channel integration |
| 39 | [`demo39_workflow`](examples/demo39_workflow.rs) | Graph workflow engine |
| 40 | [`demo40_snapshot`](examples/demo40_snapshot.rs) | Snapshot & rollback |
| 41 | [`demo41_web_tools`](examples/demo41_web_tools.rs) | Web search + fetch |
| 42 | [`demo42_playwright_mcp`](examples/demo42_playwright_mcp.rs) | Playwright MCP browser automation |
| 43 | [`demo43_data_tools`](examples/demo43_data_tools.rs) | Excel / CSV / Word / Text processing |

Plus **6 comprehensive examples** demonstrating real-world use cases:

| Example | Scenario |
|---------|----------|
| [`demo44_code_laboratory`](examples/demo44_code_laboratory.rs) | Code execution assistant |
| [`demo45_customer_service`](examples/demo45_customer_service.rs) | Intelligent customer service |
| [`demo46_data_analyst`](examples/demo46_data_analyst.rs) | Data analysis assistant |
| [`demo47_enterprise`](examples/demo47_enterprise.rs) | Enterprise workflow automation |
| [`demo48_personal_assistant`](examples/demo48_personal_assistant.rs) | Personal smart assistant |
| [`demo49_research_agent`](examples/demo49_research_agent.rs) | Research & report assistant |

---

## Compatibility

Any **OpenAI-compatible** API, plus native Anthropic and Ollama:

| Provider | Endpoint | Notes |
|----------|---------|-------|
| OpenAI | `https://api.openai.com/v1` | GPT-4o, GPT-4-turbo |
| Anthropic | `https://api.anthropic.com/v1` | Native Claude API |
| DeepSeek | `https://api.deepseek.com/v1` | DeepSeek-V3/R1 |
| Alibaba Qwen | `https://dashscope.aliyuncs.com/compatible-mode/v1` | Qwen3-max, Qwen-plus |
| Ollama (local) | `http://localhost:11434` | Native protocol |
| LM Studio | `http://localhost:1234/v1` | Any GGUF model |

---

## Documentation

| Topic | English | Chinese |
|-------|---------|---------|
| ReAct Agent | [EN](docs/en/01-react-agent.md) | [ZH](docs/zh/01-react-agent.md) |
| Tool System | [EN](docs/en/02-tools.md) | [ZH](docs/zh/02-tools.md) |
| Memory System | [EN](docs/en/03-memory.md) | [ZH](docs/zh/03-memory.md) |
| Context Compression | [EN](docs/en/04-compression.md) | [ZH](docs/zh/04-compression.md) |
| Human-in-the-Loop | [EN](docs/en/05-human-loop.md) | [ZH](docs/zh/05-human-loop.md) |
| Multi-Agent | [EN](docs/en/06-subagent.md) | [ZH](docs/zh/06-subagent.md) |
| Skill System | [EN](docs/en/07-skills.md) | [ZH](docs/zh/07-skills.md) |
| MCP Protocol | [EN](docs/en/08-mcp.md) | [ZH](docs/zh/08-mcp.md) |
| DAG Tasks | [EN](docs/en/09-tasks.md) | [ZH](docs/zh/09-tasks.md) |
| Streaming | [EN](docs/en/10-streaming.md) | [ZH](docs/zh/10-streaming.md) |
| Structured Output | [EN](docs/en/11-structured-output.md) | [ZH](docs/zh/11-structured-output.md) |
| Mock Testing | [EN](docs/en/12-mock.md) | [ZH](docs/zh/12-mock.md) |
| IM Channels | [EN](docs/en/15-im-channels.md) | [ZH](docs/zh/15-im-channels.md) |
| Plan-and-Execute | [EN](docs/en/16-plan-execute.md) | [ZH](docs/zh/16-plan-execute.md) |
| Graph Workflow | [EN](docs/en/17-graph-workflow.md) | [ZH](docs/zh/17-graph-workflow.md) |
| Guard System | [EN](docs/en/18-guard-system.md) | [ZH](docs/zh/18-guard-system.md) |
| Self-Reflection | [EN](docs/en/19-self-reflection.md) | [ZH](docs/zh/19-self-reflection.md) |

---

## Contributing

Contributions are welcome! See [CONTRIBUTING.md](./CONTRIBUTING.md) for guidelines.

**Before submitting a PR, please run locally:**

```bash
git clone https://github.com/EchoYue-lp/echo-agent
cd echo-agent

# Code formatting
cargo fmt --check

# Linting
cargo clippy --workspace --all-targets

# Tests
cargo test --workspace
```

---

## Changelog

See [CHANGELOG.md](./CHANGELOG.md) for release history.

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
