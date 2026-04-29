# echo-core

Core traits and types for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Contents

- **Agent traits**: `Agent`, `ReActAgent`, `SubAgent`
- **LLM abstractions**: `LlmClient`, `ChatRequest`, `ChatResponse`, `TokenUsage`
- **Tool system**: `Tool`, `TypedTool`, `ToolResult`, `ToolPermission`
- **Error handling**: `AgentError`, unified error types
- **Guard system**: Content filtering traits (`Guard`, `GuardResult`)
- **Retry**: `RetryPolicy` with exponential backoff
- **Project rules**: `.claude/rules` parsing

## Usage

```rust
use echo_core::prelude::*;
```

## License

MIT
