# Context 系统

## 概述

Context 系统负责在每次 LLM 调用前组装上下文消息列表。它提供两个核心组件：

- **ContextAssembler** - 集中化消息列表构建，支持预算感知
- **ContextSelector** - 基于任务相关性评分选择文件

这些组件确保 LLM 调用包含最相关的上下文，同时遵守 token 预算限制。

---

## ContextAssembler

`ContextAssembler` 负责从多个来源收集上下文，并按照优先级排序组装成消息列表。

### 上下文来源

```rust
pub struct ContextSources {
    pub system_prompt: Option<String>,           // 系统提示词
    pub project_instructions: Vec<String>,        // 项目指令
    pub user_instructions: Vec<String>,           // 用户指令
    pub conversation_history: Vec<Message>,       // 对话历史
    pub memory_recall: Vec<Message>,              // 记忆召回
    pub tool_results: Vec<Message>,               // 工具结果
    pub file_contents: Vec<Message>,              // 文件内容
    pub subagent_reports: Vec<Message>,           // 子代理报告
    pub hook_injected: Vec<Message>,              // Hook 注入
    pub task_state: Option<String>,               // 任务状态
}
```

### 优先级排序

消息按照以下优先级排序（从高到低）：

1. **Critical (10)** - 系统提示词、项目指令
2. **High (8)** - 用户指令、任务状态
3. **Medium (5)** - 对话历史、工具结果
4. **Low (3)** - 记忆召回、子代理报告
5. **BestEffort (1)** - 文件内容、Hook 注入

### 基本用法

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

### 预算配置

使用 `ContextBudget` 限制每个来源的 token 使用：

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
    conversation_history: vec![/* 大量历史消息 */],
    memory_recall: vec![/* 记忆召回 */],
    tool_results: vec![/* 工具结果 */],
    ..Default::default()
};

// Assembler 会自动截断低优先级内容以符合预算
let messages = assembler.assemble(&sources);
```

### 预算感知截断

当总 token 超过预算时，`ContextAssembler` 按以下顺序截断：

1. 首先截断 `BestEffort` 内容（文件内容）
2. 然后截断 `Low` 内容（记忆召回、子代理报告）
3. 然后截断 `Medium` 内容（对话历史、工具结果）
4. 保留 `High` 和 `Critical` 内容

截断时从最旧的内容开始，保留最新的内容。

### 与 ReactAgent 集成

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

`ContextSelector` 基于任务描述评分文件相关性，用于自动选择最相关的文件作为上下文。

### 评分策略

```rust
pub struct ContextSelector {
    pub symbol_weight: f64,      // 符号匹配权重（默认 1.0）
    pub recency_weight: f64,     // 最近修改权重（默认 0.6）
    pub git_diff_weight: f64,    // Git 变更权重（默认 0.8）
    pub max_files: usize,        // 最大文件数（默认 10）
}
```

### 评分算法

每个文件的得分 = 符号匹配得分 + 最近修改得分 + Git 变更得分

- **符号匹配**：文件名或内容包含任务关键词时加分
- **最近修改**：最近 24 小时内修改过的文件加分
- **Git 变更**：未提交的 Git 变更文件加分

### 基本用法

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

// 返回按相关性排序的文件列表
// 例如: [PathBuf::from("src/main.rs"), PathBuf::from("src/lib.rs"), ...]
```

### 自定义权重

```rust
use echo_agent::context::ContextSelector;

// 优先选择符号匹配的文件
let selector = ContextSelector {
    symbol_weight: 2.0,
    recency_weight: 0.3,
    git_diff_weight: 0.5,
    max_files: 5,
};
```

### 与代码搜索集成

```rust
use echo_agent::context::ContextSelector;
use echo_agent::tools::CodeSearchTool;

let selector = ContextSelector::new();
let search_tool = CodeSearchTool::new();

// 搜索相关文件
let search_results = search_tool.search("fn process_request")?;

// 提取文件路径
let files: Vec<PathBuf> = search_results
    .iter()
    .map(|r| PathBuf::from(&r.file))
    .collect();

// 评分并选择最相关的文件
let selected = selector.select_files(&files, &[], &[], &[], "Fix the request processing bug");
```

---

## 最佳实践

### 1. 合理设置预算

根据模型的最大上下文长度设置预算：

```rust
// 对于 8K 上下文的模型
let budget = ContextBudget {
    total_tokens: 7000,  // 保留 1000 token 用于模型响应
    user_reserve: 500,
    history_max: 2500,
    tool_results_max: 2000,
    memory_max: 1000,
    file_contents_max: 1000,
    subagent_reports_max: 500,
};

// 对于 128K 上下文的模型
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

### 2. 优先保留关键上下文

确保系统提示词和项目指令始终包含：

```rust
let sources = ContextSources {
    system_prompt: Some("You are an expert Rust developer.".to_string()),
    project_instructions: vec![
        "Always use idiomatic Rust code".to_string(),
        "Prefer Result over panic".to_string(),
    ],
    // 其他上下文...
};
```

### 3. 使用 ContextSelector 减少无关内容

在读取文件前使用 `ContextSelector` 过滤：

```rust
let selector = ContextSelector::new();
let relevant_files = selector.select_files(&all_files, &symbols, &recent, &git_changed, &task);

// 只读取相关文件
for file in relevant_files {
    let content = std::fs::read_to_string(&file)?;
    // 添加到上下文...
}
```

### 4. 动态调整预算

根据任务复杂度动态调整预算：

```rust
let budget = if task_is_complex {
    // 复杂任务需要更多历史和上下文
    ContextBudget {
        total_tokens: 15000,
        history_max: 6000,
        file_contents_max: 5000,
        ..Default::default()
    }
} else {
    // 简单任务使用较少上下文
    ContextBudget {
        total_tokens: 4000,
        history_max: 1500,
        file_contents_max: 1000,
        ..Default::default()
    }
};
```

---

## 调试技巧

### 查看组装结果

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

### 估算 token 使用

```rust
fn estimate_tokens(text: &str) -> usize {
    // 粗略估算：每 4 个字符约等于 1 个 token
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

## API 参考

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

## 示例

- [demo65_context_assembler.rs](../examples/demo65_context_assembler.rs) - ContextAssembler 完整示例
- [demo66_context_selector.rs](../examples/demo66_context_selector.rs) - ContextSelector 文件选择示例

---

## 版本历史

- **v0.2.1** (2026-05-25) - 初始版本，添加 ContextAssembler 和 ContextSelector
