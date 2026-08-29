# echo-agent Documentation

echo-agent is a composable Agent development framework written in Rust, providing a ReAct execution engine, tool system, dual-layer memory, context compression, human-in-the-loop, multi-Agent orchestration, Skill system, MCP protocol integration, and more.

> **中文文档** → [docs/zh/README.md](../zh/README.md)

---

## Documentation Index

### Core Features

| Doc                                                 | Module            | Key Concepts                                                                              |
| --------------------------------------------------- | ----------------- | ----------------------------------------------------------------------------------------- |
| [01 - ReAct Agent](./01-react-agent.md)             | Core engine       | Thought→Action→Observation, CoT, parallel tool calls, callbacks                           |
| [02 - Tool System](./02-tools.md)                   | Tools             | Tool trait, ToolManager, timeout/retry, concurrency limiting                              |
| [03 - Memory System](./03-memory.md)                | Memory            | Store (long-term), RuntimeStateStore (runtime checkpoint), ConversationStore (transcript) |
| [04 - Context Compression](./04-compression.md)     | Compression       | SlidingWindow, Summary, Hybrid pipeline, ContextManager                                   |
| [05 - Human-in-the-Loop](./05-human-loop.md)        | HIL               | Approval gate, Console/Webhook/WebSocket providers                                        |
| [06 - Multi-Agent Orchestration](./06-subagent.md)  | Subagent          | Orchestrator/Subagent/Planner, context isolation                                          |
| [07 - Skill System](./07-skills.md)                 | Skills            | Capability packs, prompt injection, external SKILL.md loading                             |
| [08 - MCP Integration](./08-mcp.md)                 | MCP               | stdio/HTTP transport, tool adaptation, multi-server management                            |
| [09 - Task Planning](./09-tasks.md)                 | Tasks / DAG       | DAG, topological sort, cycle detection, Mermaid visualization                             |
| [10 - Streaming Output](./10-streaming.md)          | Streaming         | execute_stream, AgentEvent, SSE, TTFT                                                     |
| [11 - Structured Output](./11-structured-output.md) | Structured Output | ResponseFormat, JsonSchema, extract(), extract_json()                                     |
| [12 - Mock Testing Utilities](./12-mock.md)         | Testing           | MockLlmClient, MockTool, MockAgent, InMemoryStore                                         |
| [13 - Multi-Turn Chat](./13-chat.md)                | Chat              | chat(), chat_stream(), cross-turn memory, reset()                                         |
| [14 - Semantic Search](./14-semantic-search.md)     | Semantic Search   | EmbeddingStore, Embedder, vector index, cosine similarity                                 |
| [15 - IM Channels](./15-im-channels.md)             | IM Channels       | QQ Bot / Feishu integration, WebSocket / Webhook, ChannelPlugin, message routing          |

### Advanced Features (v1.0.0)

| Doc                                                      | Module                 | Key Concepts                                                                                                                           |
| -------------------------------------------------------- | ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| [17 - Graph Workflow](./17-graph-workflow.md)            | Workflow               | LangGraph-style, SharedState, conditional edges, fan-out/fan-in                                                                        |
| [18 - Guard System](./18-guard-system.md)                | Guards                 | RuleGuard, LlmGuard, input/output filtering                                                                                            |
| [20 - Web Tools](./20-web-tools.md)                      | Web Search / Fetch     | DuckDuckGo / Brave / Tavily search, HTML→text                                                                                          |
| [21 - Common Tools](./21-common-tools.md)                | Tool Guide             | Web search, web fetch, browser automation, data tools                                                                                  |
| [22 - Research Tools](./22-research-tools.md)            | Research               | ArXiv search, Semantic Scholar, PDF fetch, BibTeX generation                                                                           |
| [23 - Hooks System](./23-hooks.md)                       | Hooks                  | Skills hooks (31 events, 7 actions), Task hooks, Subagent hooks                                                                        |
| [24 - Eval System](./24-eval-system.md)                  | Eval                   | EvalCase, SuccessCriteria, LlmGrader, A/B comparison, regression, HTML reports                                                         |
| [25 - Self-Evolution](./25-self-improvement.md)          | Improve / Evolution    | Analyzer, ImprovementLoop, EvalDrivenImprovement, tiered memory, skill auto-creation, merge/health/patch, rule promotion, change audit |
| [26 - Multi-Agent Patterns](./26-multi-agent.md)         | Subagent / Team intent | Single dispatch modes and TeamSpec-to-runtime-DAG collaboration                                                                        |
| [27 - Tracing System](./27-tracing.md)                   | Trace                  | Run, RunEvent (11 types), RunStore, JsonlRunStore, lifecycle, secret redaction                                                         |
| [28 - Config Reference](./28-config-reference.md)        | Config                 | AgentConfig, ReactAgentBuilder, ToolExecutionConfig, TokenBudgetConfig, YAML config, feature flags                                     |
| [29 - Runtime & Task System](./29-long-running-tasks.md) | Runtime & Tasks        | Unified runtime, execution serialization, DAG orchestration, ProgressBridge, background tasks, scheduling                              |

### New Features (v0.2.1)

| Doc                                                               | Module                                         | Key Concepts                                                                            |
| ----------------------------------------------------------------- | ---------------------------------------------- | --------------------------------------------------------------------------------------- |
| [30 - ReAct Safety](./30-react-safety.md)                         | Loop Detection / Adaptive Compression          | Loop detection, 5-level adaptive compression, Git checkpoint                            |
| [31 - LSP Integration](./31-lsp-integration.md)                   | LSP                                            | Language Server Protocol, code navigation, diagnostics, rust-analyzer                   |
| [32 - Plugin System](./32-plugin-system.md)                       | Plugin                                         | PluginManifest, PluginRegistry, PluginScope, lifecycle management                       |
| [33 - Headless Mode](./33-headless-mode.md)                       | Headless                                       | Non-interactive execution, CI/CD integration, JSON output, exit_code                    |
| [34 - Git Isolation](./34-git-isolation.md)                       | Git Worktree / Checkpoint                      | Parallel subagent isolation, worktree management, file operation rollback               |
| [35 - Pipelines](./35-pipelines.md)                               | Data Pipeline / Writing Pipeline               | Reproducible code-first analysis, writing quality loop                                  |
| [36 - Data Quality & Statistics](./36-data-quality-statistics.md) | Data Quality / Statistics                      | Data profiling, anomaly detection, descriptive stats, correlation analysis              |
| [37 - Code Search](./37-code-search.md)                           | Code Search                                    | Ripgrep, structured output, glob/type filtering, 50KB cap                               |
| [38 - Agent Factory & Modes](./38-factory-modes.md)               | Agent Factory / Mode Engine / Prompt Templates | mode switching, localization, template rendering                                        |
| [40 - Context System](./40-context-system.md)                     | Context System                                 | ContextAssembler, ContextBudgeter, ContextSelector, priority ordering, budget awareness |
| [41 - Persistence Concepts](./41-persistence-concepts.md)         | Store / Journal / Checkpoint / Trace           | persistence boundaries, fact authority, recovery snapshots, observability               |

### Getting Started Guides

| Doc                                           | Description                                    |
| --------------------------------------------- | ---------------------------------------------- |
| [Getting Started](./getting-started.md)       | Build your first Agent from scratch            |
| [Skill Authoring Guide](./skill-authoring.md) | Create custom Code-based and File-based Skills |

### Security

| Doc                             | Module   | Key Concepts                                                            |
| ------------------------------- | -------- | ----------------------------------------------------------------------- |
| [Security Guide](./security.md) | Security | Security model, sandbox config, secret management, MCP trust boundaries |

### Architecture Decisions

| ADR                                                                                  | Decision                                                              |
| ------------------------------------------------------------------------------------ | --------------------------------------------------------------------- |
| [0001 - Sender-Scoped Channel Sessions](../adr/0001-channel-session-sender-scope.md) | Channel sessions are isolated by channel, conversation, and sender    |
| [0006 - Runtime-State Scope Lineage](../adr/0006-runtime-state-scope-lineage.md)     | Stable scopes durably own resettable runtime checkpoint incarnations  |
| [0002 - Sandbox Cancellation Cleanup](../adr/0002-sandbox-cancellation-cleanup.md)   | Resource-owning sandbox backends drain cleanup before terminal return |
| [0007 - Atomic Journal Batch Commits](../adr/0007-atomic-journal-batch-commits.md)   | Related journal events become visible as one durable commit unit      |
| [0008 - Canonical Runtime Task Authority](../adr/0008-canonical-runtime-task-authority.md) | One revisioned graph owns task CRUD, execution, and settlement    |
| [0009 - Tracked Input Receipts](../adr/0009-tracked-input-receipts.md) | Active and initial inputs expose accepted, drained, and terminal boundaries |
| [0010 - Canonical Turn Receipt Accounting](../adr/0010-canonical-turn-receipt-accounting.md) | One framework receipt owns generic terminal, usage, compaction, and final-message facts |

---

## Quick Start

### Single-task mode (`execute`)

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("qwen3-max", "assistant", "You are a helpful assistant");
    let mut agent = ReactAgent::new(config);
    let answer = agent.execute("Explain the concept of ownership in Rust").await?;
    println!("{}", answer);
    Ok(())
}
```

### Multi-turn chat mode (`chat`)

`chat()` preserves conversation history across calls, enabling natural multi-turn dialogue.
`execute()` resets context on every call and is suited for independent single-turn tasks.

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("qwen3-max", "assistant", "You are a helpful assistant");
    let mut agent = ReactAgent::new(config);

    let r1 = agent.chat("Hi, I'm Alice and I'm a Rust developer.").await?;
    println!("Agent: {r1}");

    let r2 = agent.chat("Do you remember my name?").await?;
    println!("Agent: {r2}"); // Agent remembers "Alice" from the prior turn

    agent.reset(); // clear history, start a new session
    Ok(())
}
```

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────┐
│                   User / Application                     │
└────────────────────────┬────────────────────────────────┘
                         │ execute() / execute_stream()   (single-task, resets context)
                         │ chat()    / chat_stream()      (multi-turn, preserves context)
┌────────────────────────▼────────────────────────────────┐
│                    ReactAgent                            │
│                                                         │
│  ┌──────────────┐  ┌────────────┐  ┌─────────────────┐  │
│  │ContextManager│  │ToolManager │  │  SkillManager   │  │
│  │(compression) │  │(execution) │  │ (Skill metadata)│  │
│  └──────────────┘  └────────────┘  └─────────────────┘  │
│                                                         │
│  ┌──────────────┐  ┌────────────┐  ┌─────────────────┐  │
│  │  RuntimeState│  │   Store    │  │HumanApprovalMgr │  │
│  │ (checkpoint) │  │(long-term) │  │ (approval gate) │  │
│  └──────────────┘  └────────────┘  └─────────────────┘  │
│                                                         │
│  ┌──────────────────────────────────────────────────┐   │
│  │            Subagent Registry                      │   │
│  │  { "math_agent": Arc<AsyncMutex<Box<dyn Agent>>> │   │
│  │    "writer_agent": ... }                          │   │
│  └──────────────────────────────────────────────────┘   │
└────────────────────────┬────────────────────────────────┘
                         │ HTTP (model-selected wire protocol)
┌────────────────────────▼────────────────────────────────┐
│                  LLM Provider                            │
│   (Responses / Anthropic Messages / Chat Completions)    │
└─────────────────────────────────────────────────────────┘
```

Providers are user-defined connections. Every configured model links to one provider,
selects Responses, Anthropic Messages, or Chat Completions explicitly, and declares its
text/image/audio/video input capabilities.

---

## Feature Matrix

| Feature                       | API / Config Field                                      | Default    |
| ----------------------------- | ------------------------------------------------------- | ---------- |
| Single-task execution         | `execute()` / `execute_stream()`                        | —          |
| **Multi-turn chat**           | **`chat()` / `chat_stream()`**                          | —          |
| Tool calling                  | `enable_tool`                                           | `true`     |
| Revisioned task graph         | `task_create` / `task_update` / `task_list`             | registered |
| Subagent orchestration        | `enable_subagent`                                       | `false`    |
| Long-term memory (Store)      | `enable_memory`                                         | `false`    |
| Human-in-the-loop             | `enable_human_in_loop`                                  | `false`    |
| Chain-of-Thought prompt       | `enable_cot`                                            | `true`     |
| Context compression           | via `set_compressor()`                                  | none       |
| Thread persistence / resume   | `conversation_id` + `state_store` (`RuntimeStateStore`) | none       |
| Transcript/history projection | `conversation_id` + `ConversationStore`                 | none       |

---

## Example Files

Examples are maintained under three contracts: `Acceptance`, `Conditional acceptance`, and `Teaching`.
See `echo-agent-learning/examples/README.md` for the full classification and upkeep rules.

| Example                                   | Demonstrates                                                                                          |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `echo-agent-learning/examples/demo01_tools.rs`                | Basic tool registration and invocation                                                                |
| `echo-agent-learning/examples/demo02_tasks.rs`                | DAG task planning                                                                                     |
| `echo-agent-learning/examples/demo03_approval.rs`             | Human-in-the-loop approval                                                                            |
| `echo-agent-learning/tests/example_contracts/demo04_subagent.rs`             | Subagent orchestration                                                                                |
| `echo-agent-learning/examples/demo05_compressor.rs`           | Context compression                                                                                   |
| `echo-agent-learning/examples/demo06_mcp.rs`                  | MCP protocol integration                                                                              |
| `echo-agent-learning/examples/demo07_skills.rs`               | Skill system                                                                                          |
| `echo-agent-learning/examples/demo08_external_skills.rs`      | External SKILL.md loading                                                                             |
| `echo-agent-learning/examples/demo09_file_shell.rs`           | File and shell tools                                                                                  |
| `echo-agent-learning/examples/demo10_streaming.rs`            | Streaming output                                                                                      |
| `echo-agent-learning/examples/demo11_callbacks.rs`            | Lifecycle callbacks                                                                                   |
| `echo-agent-learning/tests/example_contracts/demo12_resilience.rs`           | Fault tolerance and retries                                                                           |
| `echo-agent-learning/examples/demo13_tool_execution.rs`       | Tool execution configuration                                                                          |
| `echo-agent-learning/examples/demo15_structured_output.rs`    | Structured output (extract / JSON Schema)                                                             |
| `echo-agent-learning/examples/demo17_chat.rs`                 | Multi-turn chat (chat / chat_stream / reset)                                                          |
| `echo-agent-learning/examples/demo18_semantic_memory.rs`      | Store semantic search (EmbeddingStore / vector retrieval)                                             |
| `echo-agent-learning/examples/demo19_guard.rs`                | Guard system (rule / LLM content filtering)                                                           |
| `echo-agent-learning/examples/demo20_audit.rs`                | Audit logging                                                                                         |
| `echo-agent-learning/examples/demo23_a2a.rs`                  | A2A protocol                                                                                          |
| `echo-agent-learning/tests/example_contracts/demo24_topology.rs`             | Multi-agent topology visualization                                                                    |
| `echo-agent-learning/examples/demo25_macros.rs`               | Macro system showcase                                                                                 |
| `echo-agent-learning/examples/demo26_provider_factory.rs`     | Dynamic LLM factory                                                                                   |
| `echo-agent-learning/examples/demo27_sqlite_memory.rs`        | SQLite persistence                                                                                    |
| `echo-agent-learning/examples/demo28_workflow.rs`             | Workflow pipeline                                                                                     |
| `echo-agent-learning/examples/demo29_sandbox.rs`              | Sandbox execution                                                                                     |
| `echo-agent-learning/tests/example_contracts/demo30_mcp_server.rs`           | MCP server mode                                                                                       |
| `echo-agent-learning/tests/example_contracts/demo31_memory_tools.rs`         | Memory tool injection                                                                                 |
| `echo-agent-learning/examples/demo32_token_budget.rs`         | Token budget control                                                                                  |
| `echo-agent-learning/examples/demo33_retry_policy.rs`         | Unified retry                                                                                         |
| `echo-agent-learning/tests/example_contracts/demo34_workflow_stream.rs`      | Workflow streaming                                                                                    |
| `echo-agent-learning/examples/demo35_dynamic_tools.rs`        | Dynamic tool management                                                                               |
| `echo-agent-learning/examples/demo36_multimodal.rs`           | Multi-modal messages                                                                                  |
| `echo-agent-learning/tests/example_contracts/demo37_declarative_workflow.rs` | YAML/JSON workflows                                                                                   |
| `echo-agent-learning/examples/demo38_im_channels.rs`          | IM channel integration                                                                                |
| `echo-agent-learning/tests/example_contracts/demo39_workflow.rs`             | Graph workflow engine                                                                                 |
| `echo-agent-learning/examples/demo40_snapshot.rs`             | Snapshot & rollback                                                                                   |
| `echo-agent-learning/examples/demo41_web_tools.rs`            | Web search + fetch                                                                                    |
| `echo-agent-learning/examples/demo42_playwright_mcp.rs`       | Playwright MCP browser automation                                                                     |
| `echo-agent-learning/tests/example_contracts/demo43_data_tools.rs`           | Data tools (Excel / CSV / Word / Text)                                                                |
| `echo-agent-learning/examples/demo44_code_laboratory.rs`      | Code execution assistant                                                                              |
| `echo-agent-learning/examples/demo45_customer_service.rs`     | Intelligent customer service                                                                          |
| `echo-agent-learning/examples/demo46_data_analyst.rs`         | Data analysis assistant                                                                               |
| `echo-agent-learning/examples/demo47_enterprise.rs`           | Enterprise workflow automation                                                                        |
| `echo-agent-learning/examples/demo48_personal_assistant.rs`   | Personal smart assistant                                                                              |
| `echo-agent-learning/examples/demo49_research_agent.rs`       | Research & report assistant                                                                           |
| `echo-agent-learning/tests/example_contracts/demo50_eval.rs`                 | Eval system: cases, criteria, constraints, trajectory replay, trigger accuracy, HTML reports          |
| `echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs`     | Self-improvement: Analyzer failure detection, Curator skill lifecycle, TrajectorySaver fine-tune data |
