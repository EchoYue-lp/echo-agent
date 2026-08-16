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
use echo_orchestration::human_loop::ConsoleHumanLoopProvider;
use echo_orchestration::tasks::{
    DefaultTaskToolPolicy, InMemoryRevisionedTaskStore, TaskRevisionService,
};
use std::sync::Arc;

let _graph = GraphBuilder::new("pipeline");
let _approval = ConsoleHumanLoopProvider;
let _tasks = TaskRevisionService::new(
    Arc::new(InMemoryRevisionedTaskStore::new()),
    Arc::new(DefaultTaskToolPolicy::default()),
);
```

## Contents

- **Workflow Engine**: Graph + DAG + Sequential workflows with YAML/JSON support
- **Human-in-the-Loop**: Approval gates via Console, Webhook, or WebSocket
- **Task Management**: revisioned task CRUD plus a single runtime DAG executor
- **Planning**: Structured plan specifications and validation
- **Scheduling**: Cron-backed scheduled tasks

## Feature Flags

| Flag | Description |
|------|-------------|
| `websocket` | Enable WebSocket-based human-loop approvals |

## License

MIT
