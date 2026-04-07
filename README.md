<div align="center">

# echo-agent

**A Production-Ready, Composable AI Agent Framework for Rust**

[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-1.0.0-brightgreen)](https://github.com/your-org/echo-agent)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![OpenAI Compatible](https://img.shields.io/badge/API-OpenAI%20Compatible-green)](https://platform.openai.com/docs/api-reference)
[![Async](https://img.shields.io/badge/runtime-tokio-blue)](https://tokio.rs/)

Build autonomous AI agents with Rust's **memory safety**, **zero-cost abstractions**, and **async-native concurrency**.

[中文文档](./README.zh.md) · [Documentation](./docs/en/README.md) · [Examples](./examples/) · [Knowledge Base](./docs/knowledge/)

</div>

---

## Why echo-agent?

Most AI agent frameworks are written in Python. **echo-agent** brings the full power of a modern agent framework to Rust — matching feature parity with LangGraph, CrewAI, and AutoGen while delivering the performance and reliability only Rust can offer.

### The Rust Advantage

| Python Frameworks | echo-agent (Rust) |
|-------------------|-------------------|
| Runtime errors from typos | Compile-time safety |
| GIL limits concurrency | True async parallelism |
| Memory leaks possible | Guaranteed memory safety |
| Slow startup (imports) | Instant binary startup |
| Deployment complexity | Single binary deploy |

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

## Highlights

| Metric | Value |
|--------|-------|
| **Capabilities** | 30+ modules: ReAct, Tools, Memory, Streaming, Multi-Agent, Skills, MCP, Guards, Audit... |
| **Examples** | 39 runnable demos — every feature has a `cargo run --example demoXX` |
| **Tests** | 350+ unit tests with comprehensive coverage |
| **Crates** | 5 modular crates, 1 simple import `use echo_agent::prelude::*` |
| **Source Files** | 154 Rust files across workspace |
| **Documentation** | Bilingual (EN/ZH) + 14 feature guides |

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         User / Application                               │
└────────────────────────────────────┬────────────────────────────────────┘
                                     │
                                     │  execute()     → single task (reset context)
                                     │  chat()        → multi-turn (preserve history)
                                     │  execute_stream() / chat_stream() → real-time events
                                     │
┌────────────────────────────────────▼────────────────────────────────────┐
│                          ReactAgent Engine                               │
│                                                                         │
│   ┌─────────────────┐   ┌─────────────────┐   ┌─────────────────────┐  │
│   │ ContextManager  │   │   ToolManager   │   │    SkillRegistry    │  │
│   │ (compress/trunc)│   │ (register/exec) │   │ (code + file skills)│  │
│   └─────────────────┘   └─────────────────┘   └─────────────────────┘  │
│                                                                         │
│   ┌─────────────────┐   ┌─────────────────┐   ┌─────────────────────┐  │
│   │   Checkpointer  │   │     Store       │   │   GuardManager      │  │
│   │ (session history)│   │  (long-term KV) │   │  (input/output filter)│  │
│   └─────────────────┘   └─────────────────┘   └─────────────────────┘  │
│                                                                         │
│   ┌─────────────────────────────────────────────────────────────────┐  │
│   │                     SubAgent Registry                            │  │
│   │   orchestrator → math_agent / writer_agent / researcher_agent   │  │
│   └─────────────────────────────────────────────────────────────────┘  │
│                                                                         │
│   ┌─────────────────┐   ┌─────────────────┐   ┌─────────────────────┐  │
│   │  AuditLogger    │   │  PermissionSvc  │   │    McpManager       │  │
│   │ (event logging) │   │ (tool approval) │   │  (external tools)   │  │
│   └─────────────────┘   └─────────────────┘   └─────────────────────┘  │
└────────────────────────────────────┬────────────────────────────────────┘
                                     │
                                     │  HTTP (OpenAI-compatible API)
                                     │
┌────────────────────────────────────▼────────────────────────────────────┐
│                        LLM Provider Layer                               │
│                                                                         │
│   ┌──────────────┐   ┌──────────────┐   ┌──────────────┐   ┌─────────┐ │
│   │   OpenAI     │   │  Anthropic   │   │   Ollama     │   │  Qwen   │ │
│   │  (GPT-4o)    │   │  (Claude)    │   │  (Local)     │   │(DeepSeek)│ │
│   └──────────────┘   └──────────────┘   └──────────────┘   └─────────┘ │
│                                                                         │
│   Unified RetryPolicy + ProviderFactory for hot-swapping               │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Core Features

### Agent Execution Strategies

| Strategy | Pattern | Use Case |
|----------|---------|----------|
| **ReAct** | Thought → Action → Observation | Interactive reasoning, tool orchestration |
| **Plan-and-Execute** | Planner → Executor → Summary | Structured multi-step tasks with DAG |
| **Self-Reflection** | Generate → Critique → Refine | Quality-guaranteed outputs with learning |

### Memory System (Dual-Layer)

| Layer | Trait | Storage | Scope |
|-------|-------|---------|-------|
| **Short-term** | `Checkpointer` | File / InMemory | Session conversation history |
| **Long-term** | `Store` | File / InMemory / SQLite / Embedding | Namespace-isolated KV store |
| **Semantic** | `EmbeddingStore` | Vector index | Cosine similarity retrieval |

### Tool System

```rust
// Single macro = Params struct + JSON Schema + TypedTool impl
#[tool(name = "search_web", description = "Search the web for information")]
async fn search_web(
    /// Search query
    query: String,
    /// Maximum results (default: 5)
    #[schemars(default = "default_limit")]
    limit: usize,
) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("Results for: {}", query)))
}

fn default_limit() -> usize { 5 }
```

**Features:**
- Auto JSON Schema generation via `schemars`
- Timeout + retry + concurrency limiting (`ToolExecutionConfig`)
- Dynamic tool management (`add_tool`, `remove_tool`, `replace_tool`)
- Permission model with pluggable policies

### Graph Workflow (LangGraph-style)

```rust
use echo_agent::workflow::{GraphBuilder, SharedState};

let graph = GraphBuilder::new("research_pipeline")
    // Agent nodes with input/output key mapping
    .add_agent_node("researcher", researcher_agent)
        .input_key("task")
        .output_key("research")
    .add_agent_node("writer", writer_agent)
        .input_key("research")
        .output_key("result")
    // Function nodes for data transformation
    .add_function_node("format", |state| Box::pin(async move {
        let result: String = state.get("result").unwrap_or_default();
        state.set("final", format!("### Report\n\n{}", result));
        Ok(())
    }))
    // Graph structure
    .set_entry("researcher")
    .add_edge("researcher", "writer")
    .add_edge("writer", "format")
    .set_finish("format")
    .build()?;

// Execute with real-time events
let mut stream = graph.run_stream(SharedState::new()).await?;
while let Some(event) = stream.next().await {
    match event? {
        WorkflowEvent::NodeStart { node_name, .. } => println!("▶ {node_name}"),
        WorkflowEvent::NodeEnd { elapsed, .. } => println!("✓ done ({elapsed:?})"),
        WorkflowEvent::Completed { result, .. } => println!("Final: {result}"),
        _ => {}
    }
}
```

**Workflow Types:**
- `Graph` — LangGraph-style DAG with conditional edges
- `SequentialWorkflow` — Simple pipeline
- `ConcurrentWorkflow` — Parallel execution
- `DagWorkflow` — Topological scheduling

### MCP Protocol Integration

```rust
use echo_agent::mcp::{McpManager, McpServerConfig};

let mut mcp = McpManager::new();

// Connect stdio MCP server (filesystem access)
let tools = mcp.connect(McpServerConfig::stdio(
    "filesystem",
    "npx", vec!["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
)).await?;

agent.add_tools(tools);

// Connect HTTP MCP server
let tools = mcp.connect(McpServerConfig::http(
    "api-server",
    "http://localhost:3000/mcp"
)).await?;
```

**Transport Support:**
- stdio (local process)
- SSE (Server-Sent Events)
- HTTP (REST API)

### Skill System (agentskills.io aligned)

```rust
// Code-based skill: Tool bundle + optional prompt injection
pub struct CalculatorSkill;

impl Skill for CalculatorSkill {
    fn name(&self) -> &str { "calculator" }
    fn description(&self) -> &str { "Mathematical calculations" }
    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![Box::new(AddTool), Box::new(MultiplyTool)]
    }
    fn system_prompt_injection(&self) -> Option<String> {
        Some("You have access to a calculator for precise math.".into())
    }
}

// File-based skill (SKILL.md):
// skills/code_review/SKILL.md
// → Progressive disclosure: discover → activate → use
```

### Human-in-the-Loop

```rust
use echo_agent::human_loop::{ConsoleHumanLoopProvider, WebSocketHumanLoopProvider};

// Console approval (terminal prompt)
let provider = ConsoleHumanLoopProvider::new();
agent.set_human_loop_provider(Arc::new(provider));

// WebSocket approval (web UI)
let provider = WebSocketHumanLoopProvider::new("ws://localhost:8080/approval");
agent.set_human_loop_provider(Arc::new(provider));
```

### Guard System

```rust
// Rule-based guard (instant)
let guard = RuleGuardBuilder::new("no-pii")
    .block_regex(r"\b\d{3}-\d{2}-\d{4}\b") // SSN pattern
    .block_regex(r"\b[A-Z]{2}\d{6}\b")     // Passport pattern
    .build();

// LLM-based guard (semantic)
let llm_guard = LlmGuard::new("qwen3-max")
    .prompt("Check if content contains sensitive information");

agent.set_guard_manager(GuardManager::new()
    .add_input_guard(Box::new(guard))
    .add_output_guard(Box::new(llm_guard)));
```

### Structured Output

```rust
#[derive(Deserialize, JsonSchema)]
struct AnalysisResult {
    sentiment: String,
    confidence: f64,
    keywords: Vec<String>,
}

let result: AnalysisResult = agent.extract("Analyze: 'I love Rust!'").await?;
println!("Sentiment: {} ({:.0}% confidence)", result.sentiment, result.confidence * 100);
```

### Self-Reflection Agent

```rust
use echo_agent::agents::self_reflection::{SelfReflectionAgent, LlmCritic};

let generator = ReactAgentBuilder::simple("qwen3-max", "Technical writer")?;
let critic = LlmCritic::new("qwen3-max").with_pass_threshold(8.0);

let mut agent = SelfReflectionAgent::new("reflection_agent", generator, critic)
    .max_reflections(3);

// Generation → Critique (score < 8) → Reflection → Refinement → Repeat
let result = agent.execute("Explain Rust ownership clearly and accurately").await?;
```

### Plan-and-Execute Agent

```rust
use echo_agent::agents::plan_execute::{PlanExecuteAgent, LlmPlanner, ReactExecutor};

let planner = LlmPlanner::new("qwen3-max");
let executor = ReactExecutor::new(ReactAgentBuilder::simple("qwen3-max", "Executor")?);

let mut agent = PlanExecuteAgent::new("planner", planner, executor)
    .max_replans(3);

// Plan → DAG Tasks → Execute → Incremental Replan on failure → Summary
let result = agent.execute("Research, analyze, and write a report on Rust async patterns").await?;
```

### A2A Protocol (Agent-to-Agent)

```rust
use echo_agent::a2a::{AgentCard, A2AServer, A2AClient};

// Server: Publish Agent Card
let card = AgentCard::builder("translator", "http://localhost:8080")
    .description("Multi-language translation agent")
    .skill(AgentSkill::new("translate", "Translate text"))
    .streaming()
    .build();

let server = A2AServer::new(card, agent);

// Client: Discover and invoke remote agent
let client = A2AClient::new("http://remote-agent:8080");
let task_id = client.send_task("Translate to French: Hello world").await?;
```

---

## Macro System

| Macro | Input | Output | Purpose |
|-------|-------|--------|---------|
| `#[tool]` | `async fn` | `Params` + `TypedTool` | Define tool in one function |
| `#[callback]` | `impl` block | `AgentCallback` | Override lifecycle hooks |
| `#[guard]` | `async fn` | `Guard` | Content filtering rule |
| `#[handler]` | `impl` block | `HumanLoopHandler` | Approval/input handling |
| `#[compressor]` | `async fn` | `ContextCompressor` | Custom compression strategy |
| `#[audit_logger]` | `impl` block | `AuditLogger` | Event logging backend |
| `agent!{}` | key-value | `ReactAgent` | Declarative agent construction |
| `messages![]` | role-content pairs | `Vec<Message>` | Quick message list |
| `tool_params!{}` | schema DSL | `JSON Value` | JSON Schema builder |

---

## Workspace Structure

```
echo-agent/
├── echo-core/          # Core traits & types
│   ├── agent.rs        # Agent trait, AgentEvent, AgentCallback
│   ├── tools/mod.rs    # Tool, TypedTool, ToolResult, Permission
│   ├── llm/mod.rs      # LlmClient trait, Message types
│   ├── guard.rs        # Guard trait, GuardResult
│   ├── audit.rs        # AuditLogger trait
│   └── retry.rs        # RetryPolicy, with_retry, with_retry_if
│
├── echo-macros/        # Procedural macros
│   └── lib.rs          # #[tool], #[callback], #[guard], #[handler], etc.
│
├── echo-providers/     # LLM implementations
│   ├── openai.rs       # OpenAI-compatible client
│   ├── anthropic.rs    # Anthropic Claude native client
│   └── ollama.rs       # Local Ollama client
│
├── echo-mcp/           # MCP protocol
│   ├── client.rs       # McpManager, tool adaptation
│   ├── transport/      # stdio, SSE, HTTP transports
│   └── types.rs        # MCP message types
│
├── src/                # Main crate
│   ├── agents/         # Agent implementations
│   │   ├── react/      # ReAct engine
│   │   ├── plan_execute/ # Plan-and-Execute
│   │   ├── self_reflection/ # Self-Reflection
│   │   └── subagent/   # SubAgent orchestration
│   ├── memory/         # Store, Checkpointer, EmbeddingStore
│   ├── workflow/       # Graph, Sequential, Concurrent, DagWorkflow
│   ├── tools/          # ToolManager, builtin tools
│   ├── skills/         # SkillRegistry, Skill trait
│   ├── guard/          # GuardManager, RuleGuard, LlmGuard
│   ├── compression/    # SlidingWindow, Summary, Hybrid
│   ├── testing/        # MockLlmClient, MockAgent, MockTool
│   ├── a2a/            # Agent-to-Agent protocol
│   └── telemetry/      # OpenTelemetry integration
│
├── examples/           # 39 runnable demos
├── docs/               # Bilingual documentation (en/zh)
│   └ knowledge/        # Knowledge base (patterns, concepts)
├── skills/             # External skill packs (SKILL.md format)
│
└── echo-cli/           # CLI tool (optional)
```

---

## Quick Start

### Prerequisites

```bash
# Install Rust (2024 edition)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Configuration

Create `echo-agent.yaml` (or set `$ECHO_AGENT_CONFIG`):

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

### Run Examples

```bash
# Basic tools
cargo run --example demo01_tools

# Macro showcase
cargo run --example demo25_macros

# Workflow streaming
cargo run --example demo34_workflow_stream

# Multi-modal (images)
cargo run --example demo36_multimodal

# Declarative YAML workflow
cargo run --example demo37_declarative_workflow

# Self-Reflection agent
cargo run --example demo20_audit

# MCP integration
cargo run --example demo06_mcp
```

### Build & Test

```bash
cargo build --release
cargo test --lib
cargo clippy -- -D warnings
```

---

## Documentation

### Feature Guides

| Doc | Module | Key Concepts |
|-----|--------|--------------|
| [01 - ReAct Agent](docs/en/01-react-agent.md) | Core engine | Thought→Action→Observation, CoT, callbacks |
| [02 - Tool System](docs/en/02-tools.md) | Tools | TypedTool, timeout/retry, permissions |
| [03 - Memory System](docs/en/03-memory.md) | Memory | Store, Checkpointer, namespace isolation |
| [04 - Context Compression](docs/en/04-compression.md) | Compression | SlidingWindow, Summary, Hybrid |
| [05 - Human-in-the-Loop](docs/en/05-human-loop.md) | HIL | Approval, Console/WebSocket providers |
| [06 - Multi-Agent](docs/en/06-subagent.md) | SubAgent | Orchestrator, context isolation |
| [07 - Skill System](docs/en/07-skills.md) | Skills | Code/file-based, progressive disclosure |
| [08 - MCP Integration](docs/en/08-mcp.md) | MCP | stdio/HTTP transport, tool adaptation |
| [09 - Task Planning](docs/en/09-tasks.md) | Tasks | DAG, topological sort, Mermaid |
| [10 - Streaming Output](docs/en/10-streaming.md) | Streaming | AgentEvent, SSE, TTFT |
| [11 - Structured Output](docs/en/11-structured-output.md) | Structured | JsonSchema, extract() |
| [12 - Mock Testing](docs/en/12-mock.md) | Testing | MockLlmClient, MockAgent |
| [13 - Multi-Turn Chat](docs/en/13-chat.md) | Chat | chat(), reset() |
| [14 - Semantic Search](docs/en/14-semantic-search.md) | Semantic | EmbeddingStore, vectors |

### Knowledge Base

| Topic | Description |
|-------|-------------|
| [Agent Patterns](docs/knowledge/agent-patterns.md) | ReAct, Plan-and-Execute, Self-Reflection, LangGraph-style workflow |
| [MCP Protocol](docs/knowledge/mcp-protocol.md) | Model Context Protocol specification and integration |
| [Skill System Design](docs/knowledge/skill-system.md) | agentskills.io specification and progressive disclosure |
| [A2A Protocol](docs/knowledge/a2a-protocol.md) | Agent-to-Agent communication and discovery |

---

## Provider Compatibility

| Provider | Endpoint | Features |
|----------|----------|----------|
| OpenAI | `https://api.openai.com/v1` | GPT-4o, streaming, vision |
| Anthropic | `https://api.anthropic.com/v1` | Claude, native format |
| DeepSeek | `https://api.deepseek.com/v1` | DeepSeek-V3 |
| Alibaba Qwen | `https://dashscope.aliyuncs.com/compatible-mode/v1` | Qwen3-max |
| Ollama (local) | `http://localhost:11434` | Llama3, Mistral, Qwen2 |
| LM Studio | `http://localhost:1234/v1` | Local models |

---

## Feature Flags

```toml
[dependencies]
echo_agent = { version = "1.0.0", features = ["full"] }

# Or selective features:
echo_agent = { version = "1.0.0", features = ["mcp", "a2a", "sqlite"] }
```

| Feature | Description |
|---------|-------------|
| `full` | All features (default) |
| `mcp` | MCP protocol integration |
| `a2a` | Agent-to-Agent protocol |
| `sqlite` | SQLite-based Store |
| `telemetry` | OpenTelemetry tracing |
| `human-loop` | WebSocket approval |
| `plan-execute` | Plan-and-Execute agent |
| `workflow` | Graph workflow engine |
| `self-reflection` | Self-Reflection agent |
| `subagent` | SubAgent orchestration |

---

## Contributing

```bash
git clone https://github.com/your-org/echo-agent
cd echo-agent
cargo build
cargo test --lib
cargo run --example demo01_tools
```

Before submitting PRs:
- `cargo fmt && cargo clippy -- -D warnings`
- Add tests for new functionality
- Update docs in `docs/`

---

## License

MIT © echo-agent contributors