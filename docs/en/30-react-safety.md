# ReAct Safety Mechanisms

## Bounded Execution

The live ReAct loop limits iterations and provider-reported model tokens through one execution path. Invocation cancellation propagates to LLM providers, tools, context compression, and Subagents; subsystem timeouts settle their owned work. Typed tool failures provide recovery guidance without a second loop state machine.

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new("qwen3-max", "agent", "You are an assistant")
    .enable_tool(true)
    .max_iterations(30)
    .run_budget(RunBudgetPolicy {
        iteration_wind_down_remaining: Some(3),
        max_model_tokens: Some(80_000),
    })
    .token_limit(128_000);
```

## Adaptive Compression

`AdaptiveCompressor` progressively handles large tool output, old conversation history, and emergency overflow from L1 through L5. It shares the configured tokenizer with `ContextManager` and accounts for the system prompt, tool definitions, protected context, and output reserve.

```rust
use echo_agent::compression::{AdaptiveCompressionConfig, AdaptiveCompressor, ContextManager};

let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig::default());
let context = ContextManager::builder(128_000)
    .compressor(compressor)
    .build();
```

## Git Checkpoints

`git_checkpoint` creates a recoverable checkpoint before file mutations. It only acts inside a Git repository; rollback restores working-tree files without changing the branch or HEAD. Long tasks should clean up expired checkpoints.

## Composition Rules

- `max_iterations` and `RunBudgetPolicy` bound iterations and model-token use.
- `token_limit` and `TokenBudget` bound the complete provider request, not only conversation messages.
- The caller owns the cancellation token and consumes one terminal event.
- File mutations use atomic writes and Git checkpoints to avoid partial crash writes.
- Agent automation permissions do not gate user-driven terminal or MCP interactions.

## Related Documentation

- [Context Compression](04-compression.md)
- [Configuration Reference](28-config-reference.md)
- [Long-Running Tasks](29-long-running-tasks.md)
