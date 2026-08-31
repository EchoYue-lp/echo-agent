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

Active Subagent 消息使用 `SubagentExecutor::send_message_tracked`。返回的
`SubagentMessageReceipt` 只携带 exact attempt identity 与嵌套的
`AgentSteerReceipt`；mailbox 接收、上下文 drain 和所属 turn 结算只由嵌套
receipt 负责。单独的 turn ID 不代表投递完成。

`SubagentAttemptIdentity` 是 framework 所有的可序列化值，只包含逻辑 task、物理
execution 和 attempt 编号。消费者需要关联 command 或 recovery 记录时可直接持久化
此值，不需要再建立产品 identity 镜像。反序列化与 `new` 使用相同的不变量：task 和
execution 非空，attempt 必须为正数。
未知 identity 字段也会被拒绝。

`SubagentResult::usage()` 返回所有消费者共用的可序列化 `ExecutionUsage`
事实：耗时、provider 报告的总 token 和 iteration 数。产品可以把它投影到 UI，但不应
重新定义一套 Rust usage 模型。

持久化 control command 应直接使用 `SubagentCommandIdentity`。它在 exact attempt identity
外增加通用的 run、plan-revision 和幂等字段，并校验完整 command envelope。
它的 `SubagentCommandPhase` 是 durable lifecycle（`persisted`、`mailbox_accepted`、
`drained`、`turn_settled`）；进程内绑定仍由独立的 `SubagentControlPhase` 表示。

## Team 意图

`TeamSpec` 只保存已注册 Subagent 的名称，不持有 Agent 实例、关系 store 或
scheduler。应用已经拥有 `SubagentRegistry` 时，优先使用这个入口。

```rust
use echo_agent::prelude::*;

let definition = SubagentBuilder::new("review-team")
    .description("从独立视角审查改动")
    .team(TeamSpec {
        strategy: TeamStrategy::ManagerSubagent,
        manager: "review-lead".to_string(),
        subagents: vec!["correctness".to_string(), "tests".to_string()],
        config: TeamConfig {
            max_concurrent: 2,
            ..TeamConfig::default()
        },
    })
    .build();

assert_eq!(definition.name, "review-team");
```

Team definition 与所有引用成员必须注册到同一 `SubagentRegistry`。可用
`ExecutionMode::Team` 调度，也可调用 `agent_tool` 并传入 `mode: "team"`。

框架复用方也可以直接组合已有 Agent 对象，而不会产生第二条执行路径：

```rust,ignore
use echo_agent::prelude::*;

let team = TeamAgent::builder()
    .name("review-team")
    .manager("lead", lead_agent, lead_definition)
    .subagent("correctness", correctness_agent, correctness_definition)
    .subagent("tests", tests_agent, tests_definition)
    .strategy(TeamStrategy::ManagerSubagent)
    .build()?;

let answer = team.execute("审查当前改动").await?;
```

builder 为框架复用方保留对象 identity，但首次执行时会把对象注册进同一个
`SubagentRegistry`。成员任务仍统一经过 `SubagentExecutor` 的 hook、prompt
编译、取消、typed result 解析和 usage 统计。

各策略只负责生成普通任务依赖：

| 策略 | canonical graph |
|---|---|
| `ManagerSubagent` | manager 规划节点 -> revision patch -> 成员任务 -> manager 汇总 |
| `Pipeline(names)` | 按给定顺序形成依赖链；前一输出原样成为下一任务载荷 |
| `Debate { judge, debaters }` | 并行方案 -> judge 汇总 |
| `Swarm { reducer }` | 声明的成员分片 -> reducer 汇总 |

manager 必须返回 typed JSON 任务计划。未知字段、未知 Subagent、重复 task ID
和无效依赖会在 graph revision 提交前 fail-closed。Manager、debate、swarm 会把
已完成依赖的输出加入下游任务 prompt。Pipeline
保留更强的契约：前一节点的完整输出会原样成为下一成员的任务载荷，然后再由
共享 `SubagentExecutor` 应用正常的 invocation context 与 prompt policy。框架不会
从模型自由文本中推断另一套状态；每个 claim 只由 canonical
`SubagentResult.outcome.status` 结算。

## 运行时权威

生产数据流为：

```text
TeamSpec 或 TeamAgentBuilder
  -> TeamRuntime（默认内存实现，或调用方持久 adapter）
  -> TaskRevisionService
  -> RuntimeTaskService
  -> SubagentExecutor
  -> typed SubagentResult
  -> 在同一 revisioned graph 中精确结算 claim
```

`RuntimeTaskService` 唯一负责 ready frontier、派生依赖阻塞、并发 wave、取消和终态
选择。Team 代码只编译意图并提供薄 dispatch adapter。ReAct checkpoint 不再
重复保存 task node 或任务生命周期状态。

设置显式 `run_id` 后，`TeamAgent` 会保留默认内存 runtime，因此同一对象可续跑
已有 graph，不会重复调度已完成成员。没有稳定 run ID 时，每次调用使用短生命周期
runtime，不累计匿名 graph。需要跨进程持久恢复的框架复用方应在已有 revision store
与 typed result 权威之上实现 `TeamRuntime`，然后调用 `execute_team_on_runtime`。runtime
必须先持久化成功的 `SubagentResult`，才能把对应 task 暴露为 Completed。每次
恢复都会核对已存 objective 与序列化后的 `TeamSpec`；同一 run id 对应不同意图
时会 fail-closed。产品 task policy 可以扩展 metadata，但必须保留 Team 拥有的
metadata key，且不得改写依赖、成员或执行语义。

## 设计参考

这里参考了两类成熟实现：

- [OpenAI Agents SDK 多 Agent 编排](https://openai.github.io/openai-agents-python/multi_agent/)
  允许组合具体 specialist Agent，同时由唯一 Runner 负责执行。
- [LangGraph supervisor](https://github.com/langchain-ai/langgraph-supervisor-py)
  接收预构建 Agent，但把协作编译成一张图，并使用唯一 checkpointer/store 边界。

因此 echo-agent 同时保留名称式和对象式组合；revision、ready frontier、重试、
取消与结算仍由 canonical task runtime 唯一负责。已删除的 Team 私有 `TaskNode`
循环不会恢复。

## 如何选择

- 单次聚焦调用且立刻需要结果：`Sync`。
- 单次隔离调用且显式传递上下文：`Fork`。
- 调用方需要实时 join/cancel handle：`Teammate`。
- 协作具有明确成员依赖和最终汇总步骤：`Team`。
