# echo-agent Documentation

echo-agent is a composable Agent development framework written in Rust, providing a ReAct execution engine, tool system, dual-layer memory, context compression, human-in-the-loop, multi-Agent orchestration, Skill system, MCP protocol integration, and more.

> **中文文档** → [docs/zh/README.md](../zh/README.md)

---

## Documentation Index

| Doc | Module | Key Concepts |
|-----|--------|-------------|
| [01 - ReAct Agent](./01-react-agent.md) | Core engine | Thought→Action→Observation, CoT, parallel tool calls, callbacks |
| [02 - Tool System](./02-tools.md) | Tools | Tool trait, ToolManager, timeout/retry, concurrency limiting |
| [03 - Memory System](./03-memory.md) | Memory | Store (long-term), Checkpointer (short-term), namespace isolation |
| [04 - Context Compression](./04-compression.md) | Compression | SlidingWindow, Summary, Hybrid pipeline, ContextManager |
| [05 - Human-in-the-Loop](./05-human-loop.md) | HIL | Approval gate, Console/Webhook/WebSocket providers |
| [06 - Multi-Agent Orchestration](./06-subagent.md) | SubAgent | Orchestrator/Worker/Planner, context isolation |
| [07 - Skill System](./07-skills.md) | Skills | Capability packs, prompt injection, external SKILL.md loading |
| [08 - MCP Integration](./08-mcp.md) | MCP | stdio/HTTP transport, tool adaptation, multi-server management |
| [09 - Task Planning](./09-tasks.md) | Tasks / DAG | DAG, topological sort, cycle detection, Mermaid visualization |
| [10 - Streaming Output](./10-streaming.md) | Streaming | execute_stream, AgentEvent, SSE, TTFT |
| [11 - Structured Output](./11-structured-output.md) | Structured Output | ResponseFormat, JsonSchema, extract(), extract_json() |
| [12 - Mock Testing Utilities](./12-mock.md) | Testing | MockLlmClient, MockTool, MockAgent, InMemoryStore |

---

## Quick Start

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("gpt-4o", "assistant", "You are a helpful assistant");
    let mut agent = ReactAgent::new(config);
    let answer = agent.execute("Explain the concept of ownership in Rust").await?;
    println!("{}", answer);
    Ok(())
}
```

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────┐
│                   User / Application                     │
└────────────────────────┬────────────────────────────────┘
                         │ execute() / execute_stream()
┌────────────────────────▼────────────────────────────────┐
│                    ReactAgent                            │
│                                                         │
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
                         │ HTTP (OpenAI-compatible API)
┌────────────────────────▼────────────────────────────────┐
│                  LLM Provider                            │
│        (OpenAI / DeepSeek / Qwen / Ollama / ...)         │
└─────────────────────────────────────────────────────────┘
```

---

## Feature Matrix

| Feature | Config Field | Default |
|---------|-------------|---------|
| Tool calling | `enable_tool` | `true` |
| DAG task planning | `enable_task` | `false` |
| SubAgent orchestration | `enable_subagent` | `false` |
| Long-term memory (Store) | `enable_memory` | `false` |
| Human-in-the-loop | `enable_human_in_loop` | `false` |
| Chain-of-Thought prompt | `enable_cot` | `true` |
| Context compression | via `set_compressor()` | none |
| Session persistence | `session_id` + `checkpointer_path` | none |

---

## Example Files

| Example | Demonstrates |
|---------|-------------|
| `examples/demo01_tools.rs` | Basic tool registration and invocation |
| `examples/demo02_tasks.rs` | DAG task planning |
| `examples/demo03_approval.rs` | Human-in-the-loop approval |
| `examples/demo04_suagent.rs` | SubAgent orchestration |
| `examples/demo05_compressor.rs` | Context compression |
| `examples/demo06_mcp.rs` | MCP protocol integration |
| `examples/demo07_skills.rs` | Skill system |
| `examples/demo08_external_skills.rs` | External SKILL.md loading |
| `examples/demo09_file_shell.rs` | File and shell tools |
| `examples/demo10_streaming.rs` | Streaming output |
| `examples/demo11_callbacks.rs` | Lifecycle callbacks |
| `examples/demo12_resilience.rs` | Fault tolerance and retries |
| `examples/demo13_tool_execution.rs` | Tool execution configuration |
| `examples/demo14_memory_isolation.rs` | Memory and context isolation |
| `examples/demo15_structured_output.rs` | Structured output (extract / JSON Schema) |
| `examples/demo16_testing.rs` | Mock testing infrastructure (zero real LLM calls) |
