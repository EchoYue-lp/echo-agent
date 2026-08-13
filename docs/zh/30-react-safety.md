# ReAct 安全机制

## 有界执行

ReAct 主循环通过同一条运行路径限制迭代次数和 provider 上报的模型 token。调用级取消信号会传播到 LLM provider、工具、上下文压缩和 Subagent，各子系统 timeout 负责结算自身工作；工具失败通过类型化恢复信息返回模型，不依赖第二套循环状态机。

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new("qwen3-max", "agent", "你是一个助手")
    .enable_tool(true)
    .max_iterations(30)
    .run_budget(RunBudgetPolicy {
        iteration_wind_down_remaining: Some(3),
        max_model_tokens: Some(80_000),
    })
    .token_limit(128_000);
```

## 自适应压缩

`AdaptiveCompressor` 按 L1 到 L5 渐进处理大工具输出、旧对话和紧急超限。压缩与 `ContextManager` 共用配置的 tokenizer，并把 system prompt、工具定义、受保护上下文和输出余量统一计入预算。

```rust
use echo_agent::compression::{AdaptiveCompressionConfig, AdaptiveCompressor, ContextManager};

let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig::default());
let context = ContextManager::builder(128_000)
    .compressor(compressor)
    .build();
```

## Git 检查点

`git_checkpoint` 在文件变更前创建可恢复检查点。它只在 Git 仓库内工作；回滚恢复工作区文件，不改变分支或 HEAD。长任务结束后应清理过期检查点。

## 组合原则

- `max_iterations` 与 `RunBudgetPolicy` 限制迭代和模型 token 用量。
- `token_limit` 和 `TokenBudget` 限制完整 provider 请求，而不只是对话消息。
- 调用方持有取消 token，并消费唯一的终态事件。
- 文件修改使用原子写入和 Git 检查点，避免崩溃导致部分写入。
- 自动执行权限只约束 Agent 决策，不阻断用户直接操作的终端或 MCP。

## 相关文档

- [上下文压缩](04-compression.md)
- [配置参考](28-config-reference.md)
- [长任务](29-long-running-tasks.md)
