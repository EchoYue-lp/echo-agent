# echo-core

[![crates.io](https://img.shields.io/crates/v/echo_core?color=brightgreen)](https://crates.io/crates/echo_core)
[![docs.rs](https://docs.rs/echo_core/badge.svg)](https://docs.rs/echo_core)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)

Core traits and types for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Quickstart

Add to your `Cargo.toml`:

```toml
[dependencies]
echo_core = "0.1"
```

Use the prelude to import all core traits:

```rust
use echo_core::prelude::*;
use echo_core::error::AgentError;

// Implement the Agent trait
struct MyAgent;
impl Agent for MyAgent {
    fn name(&self) -> &str { "my-agent" }
}
```

## Contents

- **Agent traits**: `Agent`, `ReActAgent`, `SubAgent`
- **LLM abstractions**: `LlmClient`, `ChatRequest`, `ChatResponse`, `TokenUsage`
- **Tool system**: `Tool`, `TypedTool`, `ToolResult`, `ToolPermission`
- **Error handling**: `AgentError`, unified error types
- **Guard system**: Content filtering traits (`Guard`, `GuardResult`)
- **Retry**: `RetryPolicy` with exponential backoff
- **Project rules**: `.claude/rules` parsing

## Feature Flags

| Flag | Description |
|------|-------------|
| `reqwest` | HTTP client for remote LLM calls |
| `guard` | Regex-based content filtering |
| `permission` | Glob-based tool permission system |

## License

MIT
