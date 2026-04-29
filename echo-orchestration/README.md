# echo-orchestration

Orchestration layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Contents

- **Workflow Engine**: Graph + DAG + Sequential workflows with YAML/JSON support
- **Human-in-the-Loop**: Approval gates via Console, Webhook, or WebSocket
- **Task Management**: DAG task scheduling with hooks

## Feature Flags

- `websocket` — Enable WebSocket-based human-loop approvals

## Usage

```rust
use echo_orchestration::workflow::GraphBuilder;
use echo_orchestration::human_loop::ConsoleApproval;
use echo_orchestration::tasks::TaskManager;
```

## License

MIT
