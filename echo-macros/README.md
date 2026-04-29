# echo-macros

Procedural macros for the [echo-agent](https://crates.io/crates/echo_agent) framework.

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
