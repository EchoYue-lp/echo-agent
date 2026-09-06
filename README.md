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

[中文文档](./README.zh.md) &middot; [Documentation](./docs/en/README.md) &middot; [Examples](./echo-agent-learning/examples/) &middot; [Changelog](./CHANGELOG.md)

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

```rust,no_run
use echo_agent::prelude::*;
use echo_agent::{agent, tool};

#[tool(name = "add", description = "Add two numbers")]
async fn add(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a + b)))
}

#[tokio::main]
async fn main() -> Result<()> {
    let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
        echo_agent::error::ConfigError::MissingConfig(
            "quickstart".to_string(),
            "OPENAI_API_KEY".to_string(),
        )
    })?;
    let llm_config = LlmConfig::for_provider(
        "openai",
        "https://api.openai.com/v1",
        api_key,
        "qwen3.7-max",
        LlmApiProtocol::ChatCompletions,
    )?;
    let mut agent = agent! {
        model: "qwen3.7-max",
        llm_config: llm_config,
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
| **Multi-agent** | Subagent dispatch + teams | Graph | Crew | Conversation |
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
cargo run -p echo-agent-learning --example demo01_tools          # Custom tools
cargo run -p echo-agent-learning --example demo25_macros         # Macro system
cargo run -p echo-agent-learning --example demo34_workflow_stream # Workflow streaming
cargo run -p echo-agent-learning --example demo36_multimodal     # Multi-modal messages
cargo run -p echo-agent-learning --example demo38_im_channels --features channels  # IM channels
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
| `full` | — | Meta-feature: enables every flag listed below |
| `acp` | yes | Stable ACP v1 Agent adapter over the canonical turn runtime |
| `a2a` | yes | Agent-to-Agent protocol server and client |
| `mcp` | yes | Model Context Protocol client |
| `lsp` | yes | Language Server Protocol integration |
| `sqlite` | yes | SQLite-backed persistent state |
| `telemetry` | yes | OpenTelemetry tracing and metrics |
| `human-loop` | yes | Human-in-the-loop approval (Console/Webhook/WebSocket) |
| `topology` | yes | Multi-agent topology tracking |
| `tasks` | yes | DAG task scheduling |
| `subagent` | yes | Subagent orchestration |
| `web` | yes | Web search and page fetch |
| `media` | yes | PDF/Excel/Word/image extraction |
| `data` | yes | Polars-powered data tools |
| `statistics` | yes | Statistical analysis tools |
| `channels` | yes | QQ Bot and Feishu IM integrations |
| `git` | yes | Git operations tools |
| `database` | yes | SQL database tools |
| `rag` | yes | Retrieval-Augmented Generation |
| `chart` | yes | Chart generation tools |
| `content-guard` | yes | Content filtering guardrails |
| `project-rules` | yes | Project rule parsing |
| `shell` | yes | Shell command execution tools |
| `files` | yes | File system tools |
| `research` | yes | ArXiv, Semantic Scholar, PDF fetch, and BibTeX tools |
| `eval` | yes | Evaluation primitives |
| `improve` | yes | Self-improvement primitives |
| `testing` | yes | Public mocks and test helpers |

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
                    │  │  Skills   │  │   Subagent    │  │
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

echo-agent ships with **67 registered tools** across 8 crates. The prelude exposes common traits and builders; concrete tools use canonical paths such as `echo_agent::tools::files::*`, and feature-enabled tool sets can be installed with `echo_agent::tools::register_all_tools`.

### Core

| Feature | Description | API Preview |
|---------|-------------|-------------|
| **ReAct Engine** | Thought → Action → Observation loop with CoT | `agent.execute("task").await?` |
| **Tool System** | `#[tool]` macro with auto JSON Schema, timeout + retry | `#[tool(name = "calc")] async fn calc(...)` |
| **Memory** | `Store` (long-term KV) + `RuntimeStateStore` (crash recovery) + `ConversationStore` (transcript) | `.with_memory_tools(store)` |
| **Context Compression** | SlidingWindow / LLM Summary / Hybrid | `SlidingWindowCompressor::new(4096)` |
| **Token Budget** | Auto-truncation + pre-think compression trigger | `.max_tool_output_tokens(2000)` |
| **Unified Retry** | One `RetryPolicy` for LLM, MCP, A2A, sandbox | `with_retry(&policy, \|\| ...)` |
| **Dynamic Tools** | Add / remove / replace tools mid-conversation | `agent.remove_tool("old")` |
| **Streaming** | Real-time `AgentEvent` stream (tokens + tool calls) | `agent.execute_stream(task).await?` |
| **Structured Output** | LLM output → typed Rust structs via JSON Schema | `agent.extract::<Contact>(text)` |
| **Multi-Modal** | Text + images (base64/URL) + files in one message | `Message::user_with_image(...)` |
| **Guard System** | Rule-based / LLM-powered content filtering | `#[guard(name = "safety")] async fn ...` |
| **Permission Model** | Declarative tool permissions with unified permission service | `PermissionService::from_provider(...)` |
| **Audit Logging** | Structured events with pluggable backends | `agent.set_audit_logger(...)` |
| **Macro System** | 11 macros: `#[tool]`, `agent!{}`, `messages![]`, ... | `agent! { llm_config: config, tools: [...] }` |

### Multi-Agent & Orchestration

| Feature | Description | API Preview |
|---------|-------------|-------------|
| **Subagent** | Sync / Fork / Teammate execution modes | `agent.register_agent(sub)` |
| **Task Graph** | Revisioned task CRUD on one dependency graph | `task_create` / `task_update` / `task_list` |
| **Self-Review** | LLM-based quality critique as a tool | `ReviewTool::new(critic)` |
| **Graph Workflow** | Linear, conditional, loop, parallel fan-out/fan-in | `GraphBuilder::new("pipeline")` |
| **DAG Tasks** | Revisioned CRUD plus dependency-aware execution | `TaskRevisionService::new(...)` |
| **Declarative Workflow** | Define graphs in YAML/JSON — no Rust code needed | `Graph::from_yaml("wf.yaml")?` |

### Integrations

| Feature | Description | API Preview |
|---------|-------------|-------------|
| **ACP Agent** | Stable v1 Client-Agent adapter over `AgentTurnDriver` | `AcpAgentAdapter::new(factory)` |
| **MCP Protocol** | Connect any MCP server (stdio / SSE / HTTP) | `mcp.connect(McpServerConfig::stdio(...))` |
| **A2A Protocol** | Agent Card publishing, cross-framework collaboration | `A2AServer::bind("0.0.0.0:3000")` |
| **Skill System** | Progressive disclosure: discover → activate → use | `agent.load_skill("web_research")` |
| **IM Channels** | QQ Bot (WebSocket) & Feishu (Webhook) built-in | `ChannelManager::new()` |
| **Web Tools** | Search (DuckDuckGo/Brave/Tavily) + Page Fetch | `WebSearchTool::auto()` |
| **Research Tools** | ArXiv, Semantic Scholar, PDF fetch, BibTeX generation | `ArxivSearchTool` |
| **Media Tools** | PDF, Excel, Word, multimodal image viewing | `ViewImageTool` |
| **Data Tools** | Polars-powered filter, aggregate, transform, stats | `DataReadTool` |
| **Sandbox** | Local / Docker / K8s code execution with limits | `LocalSandbox::new()` |
| **OpenTelemetry** | Distributed tracing and metrics via OTLP | `init_telemetry(&config)` |
| **Snapshot/Rollback** | Capture & restore agent state at any point | `agent.snapshot()` / `agent.rollback(1)` |
| **Circuit Breaker** | Auto-fail-fast when LLM is down | `agent.set_circuit_breaker(config)` |

---

## Feature Flags

Default features are empty. Use `features = ["full"]` to opt in to every
feature, or select individual flags from the authoritative table above. The
`[features]` section in [`Cargo.toml`](Cargo.toml) is the source of truth.

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
├── echo-tools/          Domain tools: chart, data, database, git, media, web, rag
├── echo-agent-learning/ Non-published lessons, demos, composite examples, and facade contracts
├── src/                 Agent engine, re-exports, and facade layer
└── docs/                Framework consumer documentation (en + zh)
```

> **Note:** `echo-agent` is a library framework. For a ready-to-use application with CLI, Web UI, and WebSocket, see [echo-agent-cli](https://github.com/EchoYue-lp/echo-agent-cli).

---

## Runtime Configuration

The framework accepts typed `FrameworkConfig`, `AgentConfig`, `LlmConfig`, `PermissionMode`, and explicit `DataRoot` values. It does not discover product YAML files or choose a home-directory data root. See [Runtime Configuration](docs/en/28-config-reference.md).

---

## Highlights

- **67 registered tools** — ReAct loop, data analysis, research papers, web, media, RAG, database, and more
- **Runnable examples and a teaching crate** — framework acceptance and Rust lessons are maintained separately
- **Comprehensive unit tests** — full coverage across all modules
- **8 production crates + 1 teaching crate** — production dependencies stay one-way and lessons never enter the runtime
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
        .model("qwen3.7-max")
        .system_prompt("You are a helpful assistant")
        .build()?;
    let answer = agent.execute("What is 42 * 1337?").await?;
    println!("{answer}");
    Ok(())
}
```

Three builder presets for different needs. Presets require an explicit LLM client or config; use the fluent builder's `build()` when an application intentionally injects the model later:

```rust,no_run
use echo_agent::prelude::*;

fn main() -> echo_agent::error::Result<()> {
    // Minimal — no tools, no memory, just chat
    let _agent = ReactAgentBuilder::simple("qwen3.7-max", "Be helpful")?;

    // Standard — tools + CoT enabled
    let _agent = ReactAgentBuilder::standard("qwen3.7-max", "assistant", "Be helpful")?;

    // Full-featured — tools + memory + tasks + CoT
    let _agent = ReactAgentBuilder::full_featured("qwen3.7-max", "assistant", "Be helpful")?;
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

### 3. Memory — Store + RuntimeStateStore + ConversationStore

- **Store**: Long-term key-value storage with namespace isolation (`InMemoryStore`, `FileStore`, `SqliteStore`)
- **RuntimeStateStore**: Full runtime checkpoint (messages + plan + active skills + blocked reason) for crash recovery (`SqliteRuntimeStateStore`)
- **ConversationStore**: User-visible transcript projection persisted automatically at run finalization

One line to give your agent persistent memory — no manual tool wiring:

```rust,no_run
use echo_agent::prelude::*;
use std::sync::Arc;

fn main() -> echo_agent::error::Result<()> {
    let store = Arc::new(InMemoryStore::new());
    let _agent = ReactAgentBuilder::new()
        .model("qwen3.7-max")
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
        .model("qwen3.7-max")
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
    let tracker = TokenUsageTracker::new("gpt-5.5");
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
    let _response = with_retry(&policy, || async {
        Ok::<_, echo_agent::error::ReactError>("done")
    })
    .await?;
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
        .model("qwen3.7-max")
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
        .model("qwen3.7-max")
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

```rust,ignore
use echo_agent::prelude::*;
use echo_agent::human_loop::PermissionService;

#[tokio::main]
async fn main() {
    // Create permission service with rules
    let service = PermissionService::builder()
        .mode(PermissionMode::Default)
        .rule(PermissionRule::new(
            RuleMatcher::Permission { permission: ToolPermission::Read },
            RuleBehavior::Allow,
            RuleSource::Default,
        ))
        .build();

    let decision = service.check("shell", &json!({"command": "ls"})).await;
    // Routes through 8-stage pipeline: bypass → plan → protected → rules
    // → cache → denial → mode dispatch → post-processing
}
```

### 9. Multi-Agent Orchestration — Orchestrator + Subagent teams

Coordinate multiple specialized agents through the shared Subagent lifecycle.

Three execution modes:
- **Sync** — parent blocks until subagent returns
- **Fork** — subagent runs in background, parent continues
- **Teammate** — independent background Subagent with a join/cancel handle

```rust,ignore
// Requires feature: subagent
use echo_agent::prelude::*;

fn main() -> echo_agent::error::Result<()> {
    let math_agent = ReactAgentBuilder::new()
        .model("qwen3.7-max")
        .name("math_expert")
        .system_prompt("You solve math problems.")
        .build()?;

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3.7-max")
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
        .model("qwen3.7-max")
        .build()?;

    // Discover and activate file-based skills (SKILL.md packs):
    agent.load_skills_from_dir("./skills/web_research").await?;
    Ok(())
}
```

Pre-built skills: `code-review`, `data-analyst`, `project-stats`, `python-linter`, `web-researcher`.

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
        .model("qwen3.7-max")
        .build()?;
    agent.add_tools(tools);
    Ok(())
}
```

Supports three transports: **stdio**, **SSE**, **HTTP**.

### 12. Task Planning — Revisioned task graph tools

ReactAgent exposes revisioned task-graph tools without a separate Agent type or
parallel task state machine.

> Task APIs are part of the framework core and do not require a separate feature.

```rust,ignore
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("qwen3.7-max")
        .system_prompt("You are a research assistant.")
        .enable_tools()
        .build()?;

    // The model can create, update, and inspect the revisioned task graph.
    // Product-specific execution policy belongs in the application adapter.
    let result = agent.execute("Create a research task graph for quantum computing trends").await?;
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
        .model("qwen3.7-max")
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
        .model("qwen3.7-max")
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
    model: qwen3.7-max
    system_prompt: "You are a research assistant"
    input_key: task
    output_key: research
  - name: writer
    type: agent
    model: qwen3.7-max
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
        .model("qwen3.7-max")
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

### 21. Self-Review — Quality critique as a tool

Use ReviewTool to let agents evaluate and refine their own outputs. This follows industry best practices (Hermes, Claude Code) where reflection is a tool capability, not a separate agent type.

```rust,ignore
use echo_agent::prelude::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
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
        .system_prompt("You are a technical writer. Use the review tool to self-critique your work.")
        .enable_tools()
        .tool(Box::new(review_tool))
        .build()?;

    // Agent can now call review(task, output) to evaluate its own work
    let result = agent.execute("Write a summary of quantum computing, then review it").await?;
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
        .model("qwen3.7-max")
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
        .model("qwen3.7-max")
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
See `echo-agent-learning/examples/README.md` for the full bucketed inventory and maintenance rules.

| # | Example | Demonstrates |
|---|---------|-------------|
| 01 | [`demo01_tools`](echo-agent-learning/examples/demo01_tools.rs) | Custom tools with `#[tool]` |
| 02 | [`demo02_tasks`](echo-agent-learning/examples/demo02_tasks.rs) | DAG task planning |
| 03 | [`demo03_approval`](echo-agent-learning/examples/demo03_approval.rs) | Human-in-the-loop |
| 04 | [`demo04_subagent`](echo-agent-learning/tests/example_contracts/demo04_subagent.rs) | Multi-agent orchestration |
| 05 | [`demo05_compressor`](echo-agent-learning/examples/demo05_compressor.rs) | Context compression |
| 06 | [`demo06_mcp`](echo-agent-learning/examples/demo06_mcp.rs) | MCP tool server |
| 07 | [`demo07_skills`](echo-agent-learning/examples/demo07_skills.rs) | Built-in skills |
| 08 | [`demo08_external_skills`](echo-agent-learning/examples/demo08_external_skills.rs) | External skill loading |
| 09 | [`demo09_file_shell`](echo-agent-learning/examples/demo09_file_shell.rs) | File & shell tools |
| 10 | [`demo10_streaming`](echo-agent-learning/examples/demo10_streaming.rs) | Streaming output |
| 11 | [`demo11_callbacks`](echo-agent-learning/examples/demo11_callbacks.rs) | Lifecycle callbacks |
| 12 | [`demo12_resilience`](echo-agent-learning/tests/example_contracts/demo12_resilience.rs) | Retry & fault tolerance |
| 13 | [`demo13_tool_execution`](echo-agent-learning/examples/demo13_tool_execution.rs) | Tool execution config |
| 15 | [`demo15_structured_output`](echo-agent-learning/examples/demo15_structured_output.rs) | JSON Schema output |
| 17 | [`demo17_chat`](echo-agent-learning/examples/demo17_chat.rs) | Interactive chat |
| 18 | [`demo18_semantic_memory`](echo-agent-learning/examples/demo18_semantic_memory.rs) | Semantic memory |
| 19 | [`demo19_guard`](echo-agent-learning/examples/demo19_guard.rs) | Guard system |
| 20 | [`demo20_audit`](echo-agent-learning/examples/demo20_audit.rs) | Audit logging |
| 23 | [`demo23_a2a`](echo-agent-learning/examples/demo23_a2a.rs) | A2A protocol |
| 24 | [`demo24_topology`](echo-agent-learning/tests/example_contracts/demo24_topology.rs) | Topology visualization |
| 25 | [`demo25_macros`](echo-agent-learning/examples/demo25_macros.rs) | Macro system showcase |
| 26 | [`demo26_provider_factory`](echo-agent-learning/examples/demo26_provider_factory.rs) | Dynamic LLM factory |
| 27 | [`demo27_sqlite_memory`](echo-agent-learning/examples/demo27_sqlite_memory.rs) | SQLite persistence |
| 28 | [`demo28_workflow`](echo-agent-learning/examples/demo28_workflow.rs) | Workflow pipeline |
| 29 | [`demo29_sandbox`](echo-agent-learning/examples/demo29_sandbox.rs) | Sandbox execution |
| 30 | [`demo30_mcp_server`](echo-agent-learning/tests/example_contracts/demo30_mcp_server.rs) | MCP server mode |
| 31 | [`demo31_memory_tools`](echo-agent-learning/tests/example_contracts/demo31_memory_tools.rs) | Memory tool injection |
| 32 | [`demo32_token_budget`](echo-agent-learning/examples/demo32_token_budget.rs) | Token budget control |
| 33 | [`demo33_retry_policy`](echo-agent-learning/examples/demo33_retry_policy.rs) | Unified retry |
| 34 | [`demo34_workflow_stream`](echo-agent-learning/tests/example_contracts/demo34_workflow_stream.rs) | Workflow streaming |
| 35 | [`demo35_dynamic_tools`](echo-agent-learning/examples/demo35_dynamic_tools.rs) | Dynamic tool management |
| 36 | [`demo36_multimodal`](echo-agent-learning/examples/demo36_multimodal.rs) | Multi-modal messages |
| 37 | [`demo37_declarative_workflow`](echo-agent-learning/tests/example_contracts/demo37_declarative_workflow.rs) | YAML/JSON workflows |
| 38 | [`demo38_im_channels`](echo-agent-learning/examples/demo38_im_channels.rs) | IM channel integration |
| 39 | [`demo39_workflow`](echo-agent-learning/tests/example_contracts/demo39_workflow.rs) | Graph workflow engine |
| 40 | [`demo40_snapshot`](echo-agent-learning/examples/demo40_snapshot.rs) | Snapshot & rollback |
| 41 | [`demo41_web_tools`](echo-agent-learning/examples/demo41_web_tools.rs) | Web search + fetch |
| 42 | [`demo42_playwright_mcp`](echo-agent-learning/examples/demo42_playwright_mcp.rs) | Playwright MCP browser automation |
| 43 | [`demo43_data_tools`](echo-agent-learning/tests/example_contracts/demo43_data_tools.rs) | Excel / CSV / Word / Text processing |
| 50 | [`demo50_eval`](echo-agent-learning/tests/example_contracts/demo50_eval.rs) | Eval system: cases, criteria, constraints, replay, HTML reports |
| 51 | [`demo51_self_improvement`](echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs) | Self-improvement: Analyzer, CritiqueStore, Curator, TrajectorySaver |
| 53 | [`demo53_adaptive_compression`](echo-agent-learning/tests/example_contracts/demo53_adaptive_compression.rs) | 5-level adaptive compression (Snip→Micro→Collapse→Compact→Reactive) |
| 54 | [`demo54_headless`](echo-agent-learning/tests/example_contracts/demo54_headless.rs) | Headless mode: single-prompt CI/CD execution |
| 55 | [`demo55_lsp_tools`](echo-agent-learning/tests/example_contracts/demo55_lsp_tools.rs) | LSP tools: go-to-definition, find-references, diagnostics |
| 56 | [`demo56_plugin_system`](echo-agent-learning/examples/demo56_plugin_system.rs) | Plugin system: manifest, registry, lifecycle, scope |
| 57 | [`demo57_data_pipeline`](echo-agent-learning/tests/example_contracts/demo57_data_pipeline.rs) | Code-first data pipeline: persist script → execute → verify artifacts |
| 58 | [`demo58_git_worktree`](echo-agent-learning/examples/demo58_git_worktree.rs) | Git worktree isolation + checkpoint rollback |
| 59 | [`demo59_code_search`](echo-agent-learning/examples/demo59_code_search.rs) | Ripgrep-powered code search with structured output |
| 60 | [`demo60_data_quality`](echo-agent-learning/tests/example_contracts/demo60_data_quality.rs) | Data quality profiling + statistical analysis |
| 61 | [`demo61_agent_factory`](echo-agent-learning/examples/demo61_agent_factory.rs) | Agent factory and prompt templates |
| 62 | [`demo62_prompt_templates`](echo-agent-learning/tests/example_contracts/demo62_prompt_templates.rs) | Prompt template manager with variable substitution |
| 64 | [`demo64_tool_pipeline`](echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs) | Tool execution pipeline + approval stack |
| 65 | [`demo65_context_assembler`](echo-agent-learning/tests/example_contracts/demo65_context_assembler.rs) | ContextAssembler: budget-aware context assembly with priority ordering |
| 66 | [`demo66_context_selector`](echo-agent-learning/tests/example_contracts/demo66_context_selector.rs) | ContextSelector: score and select files by task relevance |
| 67 | [`demo67_progress`](echo-agent-learning/tests/example_contracts/demo67_progress.rs) | Progress reporting |
| 68 | [`demo68_human_gate`](echo-agent-learning/examples/demo68_human_gate.rs) | Human approval gate |
| 70 | [`demo70_scheduler`](echo-agent-learning/examples/demo70_scheduler.rs) | Task scheduling |

Plus **6 comprehensive examples** demonstrating real-world use cases, and a
small public-facade composition example:

| Example | Scenario |
|---------|----------|
| [`demo44_code_laboratory`](echo-agent-learning/examples/demo44_code_laboratory.rs) | Code execution assistant |
| [`demo45_customer_service`](echo-agent-learning/examples/demo45_customer_service.rs) | Intelligent customer service |
| [`demo46_data_analyst`](echo-agent-learning/examples/demo46_data_analyst.rs) | Data analysis assistant |
| [`demo47_enterprise`](echo-agent-learning/examples/demo47_enterprise.rs) | Enterprise workflow automation |
| [`demo48_personal_assistant`](echo-agent-learning/examples/demo48_personal_assistant.rs) | Personal smart assistant |
| [`demo49_research_agent`](echo-agent-learning/examples/demo49_research_agent.rs) | Research & report assistant |
| [`comprehensive_agent`](echo-agent-learning/examples/comprehensive_agent.rs) | Deterministic tool + ReAct composition |

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
| Rust for Contributors | — | [ZH](echo-agent-learning/docs/zh/README.md) |
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
| Graph Workflow | [EN](docs/en/17-graph-workflow.md) | [ZH](docs/zh/17-graph-workflow.md) |
| Guard System | [EN](docs/en/18-guard-system.md) | [ZH](docs/zh/18-guard-system.md) |
| Multi-Turn Chat | [EN](docs/en/13-chat.md) | [ZH](docs/zh/13-chat.md) |
| Semantic Search | [EN](docs/en/14-semantic-search.md) | [ZH](docs/zh/14-semantic-search.md) |
| Web Tools | [EN](docs/en/20-web-tools.md) | [ZH](docs/zh/20-web-tools.md) |
| Common Tools | [EN](docs/en/21-common-tools.md) | [ZH](docs/zh/21-common-tools.md) |
| Research Tools | [EN](docs/en/22-research-tools.md) | [ZH](docs/zh/22-research-tools.md) |
| Hooks System | [EN](docs/en/23-hooks.md) | [ZH](docs/zh/23-hooks.md) |
| Eval System | [EN](docs/en/24-eval-system.md) | [ZH](docs/zh/24-eval-system.md) |
| Self-Improvement | [EN](docs/en/25-self-improvement.md) | [ZH](docs/zh/25-self-improvement.md) |
| Multi-Agent Patterns | [EN](docs/en/26-multi-agent.md) | [ZH](docs/zh/26-multi-agent.md) |
| Tracing System | [EN](docs/en/27-tracing.md) | [ZH](docs/zh/27-tracing.md) |
| Config Reference | [EN](docs/en/28-config-reference.md) | [ZH](docs/zh/28-config-reference.md) |
| Runtime & Task System | [EN](docs/en/29-long-running-tasks.md) | [ZH](docs/zh/29-long-running-tasks.md) |
| ReAct Safety | [EN](docs/en/30-react-safety.md) | [ZH](docs/zh/30-react-safety.md) |
| LSP Integration | [EN](docs/en/31-lsp-integration.md) | [ZH](docs/zh/31-lsp-integration.md) |
| Plugin System | [EN](docs/en/32-plugin-system.md) | [ZH](docs/zh/32-plugin-system.md) |
| Headless Mode | [EN](docs/en/33-headless-mode.md) | [ZH](docs/zh/33-headless-mode.md) |
| Git Isolation | [EN](docs/en/34-git-isolation.md) | [ZH](docs/zh/34-git-isolation.md) |
| Pipelines | [EN](docs/en/35-pipelines.md) | [ZH](docs/zh/35-pipelines.md) |
| Data Quality & Statistics | [EN](docs/en/36-data-quality-statistics.md) | [ZH](docs/zh/36-data-quality-statistics.md) |
| Code Search | [EN](docs/en/37-code-search.md) | [ZH](docs/zh/37-code-search.md) |
| Agent Factory & Model Profiles | [EN](docs/en/38-factory-modes.md) | [ZH](docs/zh/38-factory-modes.md) |
| Security | [EN](docs/en/security.md) | [ZH](docs/zh/security.md) |
| Multilingual SDK (ACP Host available) | [EN](docs/sdk/README.md) | — |

### SDK corner

The source-built `echo-agent-sdk-host` passes the supported standard ACP v1
profile through the official Client and stdio runtime, and — with the
`sdk-core-profile` feature and an explicit state root — the negotiated
`_echo_agent/*` core extension profile (Agent/Session/Run handles, full
events with ACK/replay, restart recovery). With the additional
`sdk-extension-bridge` feature it also serves the negotiated bidirectional
extension bridge: host-language Tool, LlmClient, Store, HumanLoop, Hook,
callback, intervention, factory and custom-Agent implementations register
over the same connection and are reverse-invoked with lease, deadline,
cancellation and stream-terminal semantics (see
[docs/sdk/sdk-extension-bridge.md](docs/sdk/sdk-extension-bridge.md)). It
uses the root
`AcpAgentAdapter`, creates one framework Agent per Session, and accepts an
explicit product-neutral JSON configuration. Build it with
`cargo build -p echo-sdk-host --features sdk-core-profile --locked`; no binary
or language runtime is bundled. The TypeScript/Python/Java extension clients
are not implemented yet, so **Runnable** and full facade parity are not
claimed. Start at [docs/sdk/README.md](docs/sdk/README.md), the only SDK
entry point; the core profile reference is
[docs/sdk/sdk-core-profile.md](docs/sdk/sdk-core-profile.md).

---

## Contributing

Contributions are welcome! See [CONTRIBUTING.md](./CONTRIBUTING.md) for guidelines.

**Before submitting a PR, please run locally:**

```bash
git clone https://github.com/EchoYue-lp/echo-agent
cd echo-agent
cargo fmt --all
./scripts/verify.sh
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
