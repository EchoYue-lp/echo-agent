# 核心概念

## 阅读模型

echo-agent 使用限定概念，而不是一个全局状态对象。每个概念都有 identity、scope、owner 和
persistence boundary。Run、Revision 或 Checkpoint 等名称只有在 owner 明确时才完整。

Framework 可以组合这些概念，但组合不转移权威。例如 Turn 可以产生 Trace 并更新 transcript，
但两者都不因此成为 Turn terminal 权威。

## Identity 图

```text
Agent
  +-- Session / Conversation scope
        +-- Invocation
              +-- Turn ----> TurnReceipt
              +-- Trace Run (observation)
              +-- Tool Effect / Delivery attempt

Task revision
  +-- PlanTask claim
        +-- Subagent attempt ----> Subagent outcome

Context <- selected from transcript, checkpoint, memory, rules, and resources
```

这张图中的 identity 始终带限定。Product Run ID、invocation correlation、Turn ID、Task ID、
Subagent attempt ID 和 Trace Run ID 即使被同一请求关联，也不可互换。

## 执行概念

| 概念 | 含义与 owner | 不是什么 |
| --- | --- | --- |
| `Agent` | 实现模型准备、Context 处理、Tool loop 和 raw execute/chat 行为的配置化对象 | 不是所有 Session、Task 或 Trace 的 registry |
| `Session` | 可拥有多个 Turn 和可关闭资源的协议/channel scope | 不是模型 Context 或 transcript store |
| `Conversation` | Conversation store 使用的稳定对话/历史 scope | 不是一次 Invocation 或 Turn |
| `Invocation` | 一次调用的配置、correlation、输入和 resource guard | 不自动等同 product Run 或 Trace Run |
| `Turn` | 一次已接纳、可取消的 driven execution，其终态由 TurnReceipt 表示 | 不是 Task node 或 stream item |
| `Task` | 含 specification、dependency、claim、execution 和 settlement 的 revisioned graph unit | 不是 Todo 行或 progress event |
| `Plan` | 编译入或关联到 Task revision 的可审阅 specification artifact | 不是独立 runtime state machine 或 store |
| `Subagent` | 具有隔离 Context、限定 capability、typed control、identity 和 outcome 的 Agent attempt | 不是第二套执行角色领域 |

低层 `Agent` API 仍是有效 framework contract。Driven adapter 在需要 acceptance、tracked input、
cancellation 和 terminal receipt 时使用 Turn 生命周期。详见 [ReAct Agent](./01-react-agent.md)、
[运行时与 Task](./29-long-running-tasks.md) 和 [Subagent](./06-subagent.md)。

## Context 与持久化概念

| 概念 | 含义与 owner | 持久性与非责任 |
| --- | --- | --- |
| `Context` | 当前执行中模型可见的 bounded message 和 resource | 临时/面向模型；不是完整 transcript |
| `Checkpoint` | 由限定 runtime、workflow 或其它状态权威拥有的恢复快照 | Owner-scoped；不是 ordered Journal 或 Git commit |
| `Store` / Memory | 在 active Context 之外保留的 application 或 Agent 知识 | Scope 由 Store 决定；不是执行终态 |
| `Journal` | 在明确 journal-backed 领域内提交的 ordered fact | 在该领域持久；不是全局 event bus |
| `Projection` | 从 fact 或权威状态派生的 query、history、feed 或 UI view | 可重建或 consumer-owned；不反写 fact |
| `Trace` | 由 Trace Run 和 event 表示的执行诊断 observation | 按 RunStore policy 保留；不是 business commit |
| `Delivery` | Delivery Ledger 或显式 Outbox pattern 中有 lifecycle、ack 和 settlement 的 routed payload attempt | 追踪交付；不保证任意 effect exactly-once |
| `Effect` | File、process、network、Tool 或其它外部可见动作 | 由 executor 和cleanup path拥有，不由display event拥有 |

Context 选择和压缩详见 [Context 系统](./40-context-system.md)与[压缩](./04-compression.md)。
持久语义分别见[持久化概念](./41-persistence-concepts.md)、[Trace](./27-tracing.md)和
[Delivery Ledger](./41-delivery-ledger.md)。

## 限定 Revision、Run 与 Checkpoint

| 限定名称 | Owner | 含义 |
| --- | --- | --- |
| `TaskRevision` | TaskRevisionService | 不可变 Task graph specification 版本 |
| Plugin generation | Plugin publication lifecycle | 一个已prepare并publish的component generation |
| Runtime-state incarnation | RuntimeStateStore scope | 稳定conversation scope内可reset的recovery lineage |
| SDK schema Revision | SDK contract generator | 可重算的public inventory和protocol schema版本 |
| Trace Run | RunStore | 与 Invocation 或 Turn 关联的诊断执行记录 |

没有通用 `AgentRevision`，也没有拥有全部生命周期的全局 Run。Git 或 file Checkpoint 也与
runtime Checkpoint 分开。限定这些名称可防止一个模块的版本或恢复策略悄然成为另一模块的权威。

## 状态权威规则

1. 一个数据或转移只有一个权威 owner。
2. Adapter 转换 identity、input、event 和 error，不重新拥有 terminal、Permission 或 Task graph。
3. Cancellation request 不是terminal。Owner 只在必要producer和cleanup settlement完成后发布terminal。
4. EOF、final text、Trace event、feed position 或 UI rendering 不能单独证明成功terminal。
   Reply 和 Wait 操作消费权威result或receipt，不会通过观察output创建terminal。
5. Compression 改变模型可见 Context，不改写 transcript 或 Journal fact。
6. `clear` 和 `reset` 必须指明作用的 authority，没有隐式 clear-everything。
7. Public API 存在是 framework capability 决策，不是某个 application 当前使用它的证据。

## Framework 不拥有的事项

| 关注 | Framework 职责 | Framework 外 owner |
| --- | --- | --- |
| Product Workspace | 接受显式invocation workspace reference并提供可复用原语 | Embedding application |
| Device synchronization | 没有通用状态权威 | Embedding application 或 platform |
| Product Backend 与 authentication | 暴露 protocol/runtime integration point | Product service |
| Frontend、Desktop、GUI 或 TUI state | 产生typed event和result | Surface reducer 和 application lifecycle |
| Deployment 与 release policy | 提供binary、library、telemetry 和 diagnostics | Consumer delivery system |

详细分层规则见 [Framework 与应用边界](./39-framework-application-boundary.md)。

## 示例路由

| 概念路径 | 可执行消费者 |
| --- | --- |
| Agent 与 Tool loop | [`demo01_tools`](../../echo-agent-learning/examples/demo01_tools.rs) |
| Conversation 与 chat | [`demo17_chat`](../../echo-agent-learning/examples/demo17_chat.rs) |
| Context compression | [`demo53_adaptive_compression`](../../echo-agent-learning/tests/example_contracts/demo53_adaptive_compression.rs) |
| Task graph | [`demo02_tasks`](../../echo-agent-learning/examples/demo02_tasks.rs) |
| Subagent outcome | [`demo04_subagent`](../../echo-agent-learning/tests/example_contracts/demo04_subagent.rs) |
| MCP contract | [`demo30_mcp_server`](../../echo-agent-learning/tests/example_contracts/demo30_mcp_server.rs) |
| Eval 与 Trace consumer | [`demo50_eval`](../../echo-agent-learning/tests/example_contracts/demo50_eval.rs) |

这些文件都是 Cargo example 或 test contract。文档链接其已测源码，而不维护未编译副本。

## 继续阅读

- [Framework 架构](./architecture.md)
- [Framework 生命周期](./lifecycles.md)
- [Task 规划](./09-tasks.md)
- [Context 系统](./40-context-system.md)
- [持久化概念](./41-persistence-concepts.md)
- [ADR 0040](../adr/0040-framework-concept-documentation-authority.md)
