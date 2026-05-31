# echo-macros

[![crates.io](https://img.shields.io/crates/v/echo_macros?color=brightgreen)](https://crates.io/crates/echo_macros)
[![docs.rs](https://docs.rs/echo_macros/badge.svg)](https://docs.rs/echo_macros)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)

Procedural macros for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Quickstart

```toml
[dependencies]
echo_macros = "0.2"
```

```rust
use echo_macros::tool;
use echo_core::tools::{ToolParameters, ToolResult};

/// Adds two numbers.
#[tool(name = "add", description = "Add two integers")]
async fn add(a: i64, b: i64) -> ToolResult {
    Ok(serde_json::json!({ "result": a + b }))
}
```

## Macros

| Macro | Description |
|-------|-------------|
| `#[tool]` | Generate `TypedTool` from an async fn |
| `#[callback]` | Generate `AgentCallback` from an impl block |
| `#[guard]` | Generate `Guard` from an async fn |
| `#[handler]` | Generate `HumanLoopHandler` from an impl block |
| `#[compressor]` | Generate `ContextCompressor` from an async fn |
| `#[permission_policy]` | Generate `PermissionPolicy` from an async fn |
| `#[audit_logger]` | Generate `AuditLogger` from an impl block |

## License

MIT
