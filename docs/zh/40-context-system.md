# 上下文构建组件

`ContextAssembler` 和 `ContextSelector` 是供自定义 Agent 循环使用的框架组件。
默认 `ReactAgent` 路径直接使用 `ContextManager`，不会调用 `ContextAssembler`。

## 组装上下文

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
    system_prompt: Some("你是编码助手。".into()),
    project_rules: vec!["保留用户已有改动。".into()],
    conversation_history: vec![Message::user("检查失败原因")],
    user_message: Some(Message::user("彻底修复根因")),
    ..Default::default()
};

let messages = ContextAssembler::new()
    .with_budget(budget)
    .assemble(sources);
assert!(!messages.is_empty());
```

`assemble` 会消费 `ContextSources`。它先放稳定策略，再放动态运行时上下文，
最后放当前用户消息。各来源上限使用保守的字符估算；需要精确模型窗口门禁的调用方，
应在请求 LLM 前应用对应 tokenizer/provider policy。

## 选择相关文件

```rust
use echo_agent::context::ContextSelector;
use std::collections::HashMap;
use std::path::PathBuf;

let mut symbols = HashMap::new();
symbols.insert(PathBuf::from("src/auth.rs"), vec!["authenticate".into()]);

let selector = ContextSelector::new();
let selected = selector.select_relevant(
    "修复认证问题",
    &symbols,
    &[],
    &[PathBuf::from("src/auth.rs")],
);
assert_eq!(selected, vec![PathBuf::from("src/auth.rs")]);
```

`score_files` 返回评分，`select_relevant` 最多返回 `max_files` 个路径。
相同分数按路径排序，保证重复运行结果确定。

可运行示例见 `echo-agent-learning/tests/example_contracts/demo65_context_assembler.rs` 和
`echo-agent-learning/tests/example_contracts/demo66_context_selector.rs`。
