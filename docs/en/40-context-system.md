# Context System

## Overview

The Context system is responsible for assembling context message lists before each LLM call. It provides two core components:

- **ContextAssembler** - Centralized message list construction with budget awareness
- **ContextSelector** - Scores file relevance based on task description

These components ensure LLM calls contain the most relevant context while respecting token budget limits.

---

## ContextAssembler

`ContextAssembler` collects context from multiple sources and assembles them into a message list sorted by priority.

### Context Sources

```rust
pub struct ContextSources {
    pub system_prompt: Option<String>,           // System prompt
    pub project_instructions: Vec<String>,        // Project instructions
    pub user_instructions: Vec<String>,           // User instructions
    pub conversation_history: Vec<Message>,       // Conversation history
    pub memory_recall: Vec<Message>,              // Memory recall
    pub tool_results: Vec<Message>,               // Tool results
    pub file_contents: Vec<Message>,              // File contents
    pub subagent_reports: Vec<Message>,           // Sub-agent reports
    pub hook_injected: Vec<Message>,              // Hook injected
    pub task_state: Option<String>,               // Task state
}
```

### Priority Ordering

Messages are ordered by the following priority (high to low):

1. **Critical (10)** - System prompt, project instructions
2. **High (8)** - User instructions, task state
3. **Medium (5)** - Conversation history, tool results
4. **Low (3)** - Memory recall, sub-agent reports
5. **BestEffort (1)** - File contents, hook injected

### Basic Usage

```rust
use echo_agent::context::{ContextAssembler, ContextSources};
use echo_agent::llm::Message;

let assembler = ContextAssembler::new();

let sources = ContextSources {
    system_prompt: Some("You are a helpful assistant.".to_string()),
    conversation_history: vec![
        Message::user("What is Rust?"),
        Message::assistant("Rust is a systems programming language..."),
    ],
    ..Default::default()
};

let messages = assembler.assemble(&sources);
```

### Budget Configuration

Use `ContextBudget` to limit token usage per source:

```rust
use echo_agent::context::{ContextAssembler, ContextBudget, ContextSources};

let budget = ContextBudget {
    total_tokens: 8000,
    user_reserve: 500,
    history_max: 3000,
    tool_results_max: 2000,
    memory_max: 1000,
    file_contents_max: 1000,
    subagent_reports_max: 500,
};

let assembler = ContextAssembler::new().with_budget(budget);

let sources = ContextSources {
    system_prompt: Some("You are a coding assistant.".to_string()),
    conversation_history: vec![/* lots of history */],
    memory_recall: vec![/* memory recall */],
    tool_results: vec![/* tool results */],
    ..Default::default()
};

// Assembler automatically truncates low-priority content to fit budget
let messages = assembler.assemble(&sources);
```

### Budget-Aware Truncation

When total tokens exceed the budget, `ContextAssembler` truncates in this order:

1. First truncate `BestEffort` content (file contents)
2. Then truncate `Low` content (memory recall, sub-agent reports)
3. Then truncate `Medium` content (conversation history, tool results)
4. Preserve `High` and `Critical` content

Truncation starts from the oldest content, preserving the most recent.

### Integration with ReactAgent

```rust
use echo_agent::agent::ReactAgentBuilder;
use echo_agent::context::{ContextAssembler, ContextBudget};

let budget = ContextBudget {
    total_tokens: 8000,
    ..Default::default()
};

let assembler = ContextAssembler::new().with_budget(budget);

let agent = ReactAgentBuilder::new()
    .with_context_assembler(assembler)
    .build()?;
```

---

## ContextSelector

`ContextSelector` scores file relevance based on task description for automatic selection of the most relevant files as context.

### Scoring Strategy

```rust
pub struct ContextSelector {
    pub symbol_weight: f64,      // Symbol match weight (default 1.0)
    pub recency_weight: f64,     // Recent modification weight (default 0.6)
    pub git_diff_weight: f64,    // Git change weight (default 0.8)
    pub max_files: usize,        // Maximum files (default 10)
}
```

### Scoring Algorithm

Each file's score = symbol match score + recent modification score + git change score

- **Symbol match**: Bonus when filename or content contains task keywords
- **Recent modification**: Bonus for files modified in the last 24 hours
- **Git change**: Bonus for files with uncommitted git changes

### Basic Usage

```rust
use echo_agent::context::ContextSelector;
use std::path::PathBuf;

let selector = ContextSelector::new();

let files = vec![
    PathBuf::from("src/main.rs"),
    PathBuf::from("src/lib.rs"),
    PathBuf::from("docs/README.md"),
    PathBuf::from("Cargo.toml"),
];

let symbols = vec![
    PathBuf::from("src/main.rs"),
    PathBuf::from("src/lib.rs"),
];

let recent = vec![
    PathBuf::from("src/lib.rs"),
];

let git_changed = vec![
    PathBuf::from("Cargo.toml"),
];

let task = "Fix the compilation error in main";

let selected = selector.select_files(&files, &symbols, &recent, &git_changed, task);

// Returns files sorted by relevance
// e.g.: [PathBuf::from("src/main.rs"), PathBuf::from("src/lib.rs"), ...]
```

### Custom Weights

```rust
use echo_agent::context::ContextSelector;

// Prioritize symbol-matched files
let selector = ContextSelector {
    symbol_weight: 2.0,
    recency_weight: 0.3,
    git_diff_weight: 0.5,
    max_files: 5,
};
```

### Integration with Code Search

```rust
use echo_agent::context::ContextSelector;
use echo_agent::tools::CodeSearchTool;

let selector = ContextSelector::new();
let search_tool = CodeSearchTool::new();

// Search for relevant files
let search_results = search_tool.search("fn process_request")?;

// Extract file paths
let files: Vec<PathBuf> = search_results
    .iter()
    .map(|r| PathBuf::from(&r.file))
    .collect();

// Score and select most relevant files
let selected = selector.select_files(&files, &[], &[], &[], "Fix the request processing bug");
```

---

## Best Practices

### 1. Set Reasonable Budgets

Set budgets based on model's maximum context length:

```rust
// For 8K context models
let budget = ContextBudget {
    total_tokens: 7000,  // Reserve 1000 tokens for model response
    user_reserve: 500,
    history_max: 2500,
    tool_results_max: 2000,
    memory_max: 1000,
    file_contents_max: 1000,
    subagent_reports_max: 500,
};

// For 128K context models
let budget = ContextBudget {
    total_tokens: 120000,
    user_reserve: 2000,
    history_max: 50000,
    tool_results_max: 30000,
    memory_max: 10000,
    file_contents_max: 20000,
    subagent_reports_max: 8000,
};
```

### 2. Prioritize Critical Context

Ensure system prompt and project instructions are always included:

```rust
let sources = ContextSources {
    system_prompt: Some("You are an expert Rust developer.".to_string()),
    project_instructions: vec![
        "Always use idiomatic Rust code".to_string(),
        "Prefer Result over panic".to_string(),
    ],
    // Other context...
};
```

### 3. Use ContextSelector to Reduce Noise

Filter with `ContextSelector` before reading files:

```rust
let selector = ContextSelector::new();
let relevant_files = selector.select_files(&all_files, &symbols, &recent, &git_changed, &task);

// Only read relevant files
for file in relevant_files {
    let content = std::fs::read_to_string(&file)?;
    // Add to context...
}
```

### 4. Dynamically Adjust Budget

Adjust budget based on task complexity:

```rust
let budget = if task_is_complex {
    // Complex tasks need more history and context
    ContextBudget {
        total_tokens: 15000,
        history_max: 6000,
        file_contents_max: 5000,
        ..Default::default()
    }
} else {
    // Simple tasks use less context
    ContextBudget {
        total_tokens: 4000,
        history_max: 1500,
        file_contents_max: 1000,
        ..Default::default()
    }
};
```

---

## Debugging Tips

### View Assembly Results

```rust
let assembler = ContextAssembler::new();
let messages = assembler.assemble(&sources);

for (i, msg) in messages.iter().enumerate() {
    println!("[{}] {}: {}...", 
        i, 
        msg.role, 
        &msg.content.as_text().unwrap_or("")[..50]
    );
}
```

### Estimate Token Usage

```rust
fn estimate_tokens(text: &str) -> usize {
    // Rough estimate: ~4 characters per token
    text.len() / 4
}

let total_tokens: usize = messages
    .iter()
    .filter_map(|m| m.content.as_text())
    .map(estimate_tokens)
    .sum();

println!("Estimated tokens: {}", total_tokens);
```

---

## API Reference

### ContextAssembler

```rust
pub struct ContextAssembler {
    budget: Option<ContextBudget>,
}

impl ContextAssembler {
    pub fn new() -> Self;
    pub fn with_budget(budget: ContextBudget) -> Self;
    pub fn assemble(&self, sources: &ContextSources) -> Vec<Message>;
}
```

### ContextBudget

```rust
pub struct ContextBudget {
    pub total_tokens: usize,
    pub user_reserve: usize,
    pub history_max: usize,
    pub tool_results_max: usize,
    pub memory_max: usize,
    pub file_contents_max: usize,
    pub subagent_reports_max: usize,
}
```

### ContextSelector

```rust
pub struct ContextSelector {
    pub symbol_weight: f64,
    pub recency_weight: f64,
    pub git_diff_weight: f64,
    pub max_files: usize,
}

impl ContextSelector {
    pub fn new() -> Self;
    pub fn select_files(
        &self,
        files: &[PathBuf],
        symbols: &[PathBuf],
        recent: &[PathBuf],
        git_changed: &[PathBuf],
        task: &str,
    ) -> Vec<PathBuf>;
}
```

---

## Examples

- [demo65_context_assembler.rs](../examples/demo65_context_assembler.rs) - Complete ContextAssembler example
- [demo66_context_selector.rs](../examples/demo66_context_selector.rs) - ContextSelector file selection example

---

## Version History

- **v0.2.1** (2026-05-25) - Initial release, added ContextAssembler and ContextSelector
