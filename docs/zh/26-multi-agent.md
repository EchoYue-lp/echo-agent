# 多 Agent 编排

echo-agent 只有一个 Subagent 调度入口和一个任务关系运行时。单个已注册
Subagent 使用 `Sync`、`Fork` 或 `Teammate`；需要声明式协作时使用
`Team`，框架会把协作意图编译成版本化任务 DAG。

## 单 Subagent 模式

| 模式 | 父 Agent 行为 | 默认上下文 |
|---|---|---|
| `Sync` | 等待结果 | 全新、聚焦的上下文 |
| `Fork` | 通过自有异步调度执行 | 显式过滤的历史 |
| `Teammate` | 返回 join/cancel handle | 全新独立上下文 |

所有模式都通过 `SubagentRegistry` 解析目标并由 `SubagentExecutor` 执行。
因此工具调用与程序化调度共享 hook、取消、prompt 编译、隔离和 typed event。

## Team 意图

`TeamSpec` 只保存已注册 Subagent 的名称，不持有 Agent 实例、关系 store 或
scheduler。

```rust
use echo_agent::agent::subagent::{
    SubagentBuilder, TeamConfig, TeamSpec, TeamStrategy,
};

let definition = SubagentBuilder::new("review-team")
    .description("从独立视角审查改动")
    .team(TeamSpec {
        strategy: TeamStrategy::ManagerSubagent,
        manager: "review-lead".to_string(),
        subagents: vec!["correctness".to_string(), "tests".to_string()],
        config: TeamConfig { max_concurrent: 2 },
    })
    .build();

assert_eq!(definition.name, "review-team");
```

Team definition 与所有引用成员必须注册到同一 `SubagentRegistry`。可用
`ExecutionMode::Team` 调度，也可调用 `agent_tool` 并传入 `mode: "team"`。

各策略只负责生成普通任务依赖：

| 策略 | canonical graph |
|---|---|
| `ManagerSubagent` | manager 规划 -> 成员任务 -> manager 汇总 |
| `Pipeline(names)` | 按给定顺序形成依赖链 |
| `Debate { judge, debaters }` | 并行方案 -> judge 汇总 |
| `Swarm { reducer }` | 声明的成员分片 -> reducer 汇总 |

已完成依赖的输出会追加到下游 Subagent 的任务 prompt。框架不会从模型自由文本
中推断另一套状态；每个 claim 只由 canonical `SubagentResult.outcome.status`
结算。

## 运行时权威

生产数据流为：

```text
TeamSpec
  -> TaskRevisionService + InMemoryRevisionedTaskStore
  -> RuntimeDagExecutor
  -> SubagentExecutor
  -> typed SubagentResult
  -> 在同一 revisioned graph 中精确结算 claim
```

`RuntimeDagExecutor` 唯一负责 ready frontier、依赖阻塞、并发 wave、取消和终态
选择。Team 代码只编译意图并提供薄 dispatch adapter。ReAct checkpoint 不再
重复保存 task node 或任务生命周期状态。

## 如何选择

- 单次聚焦调用且立刻需要结果：`Sync`。
- 单次隔离调用且显式传递上下文：`Fork`。
- 调用方需要实时 join/cancel handle：`Teammate`。
- 协作具有明确成员依赖和最终汇总步骤：`Team`。
