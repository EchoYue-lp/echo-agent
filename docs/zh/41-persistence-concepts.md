# Store、Journal、Checkpoint 与 Trace

这四个概念并不处于同一抽象层：`Store` 表示某类数据的读写边界；`Journal`、`Checkpoint` 和 `Trace` 表示数据承担的不同语义角色。

## 核心区别

| 概念         | 回答的问题                         | 典型形态                 | 主要用途               |
| ------------ | ---------------------------------- | ------------------------ | ---------------------- |
| `Store`      | 数据存在哪里、如何读写？           | trait、文件或内存后端    | 持久化和查询某类数据   |
| `Journal`    | 按确定顺序发生了什么？             | append-only event stream | 重放、恢复、生成投影   |
| `Checkpoint` | 执行到某个边界时状态是什么？       | state + applied sequence | 快速恢复、避免全量重放 |
| `Trace`      | 一次执行如何运行、为何成功或失败？ | `Run` + `RunEvent`       | 调试、诊断、评估和统计 |

```text
业务事件
   │
   ├─> Journal ─> reduce/fold ─> 当前状态
   │                               │
   │                               └─> Checkpoint
   │
   └─> Trace

不同 Store 负责持久化上述数据或其它领域数据。
```

## Store：领域存储接口

框架中不存在一个能表示所有持久化数据的总 `Store` trait。名字中包含 `Store`，只能说明它是某个领域的读写边界，不能据此推断数据模型或权威性。

| 接口                 | 所有的数据                       | 作用域         |
| -------------------- | -------------------------------- | -------------- |
| `Store`              | namespace/key/value 长期记忆     | 跨会话知识     |
| `ConversationStore`  | 用户可见的消息历史投影           | 对话浏览       |
| `RuntimeStateStore`  | `AgentCheckpoint`                | ReAct 会话恢复 |
| `CheckpointStore<S>` | reducer state + applied sequence | 事件投影恢复   |
| `RunStore`           | Trace `Run`/`RunEvent`           | 执行观测       |

长期记忆 `Store` 提供 namespace 隔离、KV 读写、搜索和删除。它只是框架中的一种 Store，不是 `ConversationStore`、`RuntimeStateStore` 或 `RunStore` 的父接口。

## Journal：有序事实历史

`EventJournal<E>` 保存有序事件，而不是反复覆盖当前状态。核心约束是：

- 事件只追加；
- sequence 从 1 开始连续递增；
- 可以从指定 sequence 后顺序重放；
- 事件先提交，再由 reducer 折叠为状态；
- 文件实现可修复撕裂的尾部记录，但历史中段损坏必须报错；
- 不确定的 batch commit 结果必须先 reopen/reconcile，不能盲目重试副作用。

Journal 通常适合承担“已经发生的领域事实”。当前状态、列表或 UI DTO 可以从 Journal 投影，但不应反过来覆盖事实历史。

## Checkpoint：稳定边界的状态快照

### Reducer checkpoint

`CheckpointedReducer` 将 Journal 事件 fold 为状态，并通过 `CheckpointStore<S>` 保存：

- 已应用到的 sequence；
- 该 sequence 对应的 reducer state。

恢复时先加载 checkpoint，再只重放 Journal 尾部：

```text
Journal:     1 2 3 4 5 6 7 8 9 10
Checkpoint:             state@7
恢复:                    load@7 + replay 8..10
```

`FileCheckpointStore` 使用原子替换、schema version 和 SHA-256 digest，避免把部分写入或被篡改的合法 JSON 当成可信状态。对于 event-sourced projection，这类 checkpoint 是可重建的加速结构，不替代 Journal。

### AgentCheckpoint

`AgentCheckpoint` 是另一种 checkpoint：它保存 ReAct 继续运行所需的消息、当前 plan 文本、激活技能、blocked reason、working directory 和时间戳，并由 `RuntimeStateStore` 持久化。

恢复前会校验 assistant tool call 与 tool result 是否成对，避免恢复出 provider 无法接受的上下文，或重复执行已经完成的副作用。

`AgentCheckpoint` 只拥有 ReAct runtime state，不拥有任务 DAG 或其它应用领域状态。它也不是 `ConversationStore` 中面向用户展示的 transcript。

### 其它同名 checkpoint

框架还有其它作用域不同的 checkpoint：

- compression checkpoint：记录上下文压缩边界；
- Git checkpoint：文件修改前创建 Git tag，用于工作区回滚；
- Trace 的 `Checkpoint`/`CheckpointResumed`：记录 checkpoint 行为已经发生，不是 checkpoint 本体。

因此讨论 checkpoint 时必须说明领域和恢复来源。

## Trace：执行观测

Trace 把一次 Agent invocation 保存为 `Run`。它可包含：

- run、parent run、session、turn 和 execution identity；
- agent、model、provider；
- input、final output、error 和 status；
- LLM 调用、token usage、cache 和耗时；
- tool call/result/error；
- context compression、phase transition、测试、文件修改和 Subagent 调度；
- checkpoint 保存与恢复事件。

Trace 通过 `RunStore` 持久化，供 analyzer、eval、replay 和诊断工具消费。

Trace 与 Journal 都可能包含按时间排列的事件，但职责不同：

| 对比                 | Journal        | Trace                          |
| -------------------- | -------------- | ------------------------------ |
| 目标                 | 保存领域事实   | 解释执行行为                   |
| 是否驱动领域投影     | 通常是         | 否                             |
| 是否适合作为恢复权威 | 按领域设计决定 | 默认不是                       |
| 常见内容             | 状态变更事件   | LLM、工具、usage、timing、错误 |

如果 Trace 写入失败允许主执行继续，Trace 就不能同时承担该执行的业务提交权威。

## 设计规则

新增或修改相关能力时，先回答：

1. 这是新的持久化后端，还是新的数据语义？只有前者主要是 Store 问题。
2. 哪份数据是不可丢失的事实？需要顺序恢复时优先扩展现有 Journal。
3. Checkpoint 是 Journal 派生缓存，还是独立 runtime snapshot？必须写清权威范围和重建来源。
4. Trace 写入失败是否允许主流程继续？如果允许，就不能用 Trace 判断业务是否已提交。
5. 是否已存在同作用域的 Journal、Checkpoint、Store 或 projection？不得平行实现同一语义。

## 代码入口

- 长期记忆 `Store`：`echo-core/src/memory/store.rs`
- `ConversationStore`：`echo-core/src/memory/conversation.rs`
- Journal 与 checkpointed reducer：`echo-state/src/journal/mod.rs`
- 文件 Journal/checkpoint：`echo-state/src/journal/file.rs`
- `AgentCheckpoint` / `RuntimeStateStore`：`src/state/mod.rs`
- Trace `Run` / `RunEvent` / `RunStore`：`src/trace/mod.rs`

延伸阅读：[记忆系统](03-memory.md)、[上下文压缩](04-compression.md)、[追踪系统](27-tracing.md)、[Git 隔离](34-git-isolation.md)。
