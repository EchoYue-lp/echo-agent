# echo-orchestration

[![crates.io](https://img.shields.io/crates/v/echo_orchestration?color=brightgreen)](https://crates.io/crates/echo_orchestration)
[![docs.rs](https://docs.rs/echo_orchestration/badge.svg)](https://docs.rs/echo_orchestration)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)

Orchestration layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Quickstart

```toml
[dependencies]
echo_orchestration = "0.2"
```

```rust
use echo_orchestration::workflow::GraphBuilder;
use echo_orchestration::human_loop::ConsoleApproval;
use echo_orchestration::tasks::TaskManager;

// Build a workflow graph
let graph = GraphBuilder::new()
    .add_node("analyze", analyze_step)
    .add_node("execute", execute_step)
    .edge("analyze", "execute")
    .build()?;

// Human-in-the-loop approval
let approval = ConsoleApproval::new();

// Task scheduling with hooks
let mut tasks = TaskManager::new();
tasks.on_complete(|task| println!("Done: {}", task.id));
```

## Contents

- **Workflow Engine**: Graph + DAG + Sequential workflows with YAML/JSON support
- **Human-in-the-Loop**: Approval gates via Console, Webhook, or WebSocket
- **Task Management**: DAG task scheduling with hooks

## Feature Flags

| Flag | Description |
|------|-------------|
| `websocket` | Enable WebSocket-based human-loop approvals |

## License

MIT
