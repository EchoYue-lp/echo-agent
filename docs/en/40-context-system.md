# Context Building Blocks

`ContextAssembler` and `ContextSelector` are framework building blocks for custom
agent loops. The default `ReactAgent` path uses `ContextManager` directly; it
does not call `ContextAssembler`.

## Assemble Context

```rust
use echo_agent::context::{ContextAssembler, ContextBudget, ContextSources};
use echo_agent::llm::types::Message;

let budget = ContextBudget {
    total_tokens: 128_000,
    user_reserve: 1_000,
    history_max: 40_000,
    tool_results_max: 20_000,
    memory_max: 5_000,
};

let sources = ContextSources {
    system_prompt: Some("You are a coding assistant.".into()),
    project_rules: vec!["Preserve existing user changes.".into()],
    conversation_history: vec![Message::user("Inspect the failure")],
    user_message: Some(Message::user("Fix the root cause")),
    ..Default::default()
};

let messages = ContextAssembler::new()
    .with_budget(budget)
    .assemble(sources);
assert!(!messages.is_empty());
```

`assemble` consumes `ContextSources`. It orders stable policy first, dynamic
runtime context next, and the current user message last. Source-specific limits
use conservative character-based estimates; callers needing model-exact window
admission must apply their tokenizer/provider policy before the LLM request.

## Select Relevant Files

```rust
use echo_agent::context::ContextSelector;
use std::collections::HashMap;
use std::path::PathBuf;

let mut symbols = HashMap::new();
symbols.insert(PathBuf::from("src/auth.rs"), vec!["authenticate".into()]);

let selector = ContextSelector::new();
let selected = selector.select_relevant(
    "fix authentication",
    &symbols,
    &[],
    &[PathBuf::from("src/auth.rs")],
);
assert_eq!(selected, vec![PathBuf::from("src/auth.rs")]);
```

`score_files` exposes scores; `select_relevant` returns at most `max_files`.
Equal scores are ordered by path so repeated runs are deterministic.

See `examples/demo65_context_assembler.rs` and
`examples/demo66_context_selector.rs` for runnable examples.
