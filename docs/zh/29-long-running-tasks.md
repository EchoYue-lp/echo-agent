# 长时间运行任务

## 分离职责

echo-agent 提供两种互补机制：

| 机制 | 权威范围 | 用途 |
|------|----------|------|
| `TaskRevisionService` + `RuntimeTaskService` | 持久版本化任务图与依赖生命周期 | 多步骤 Agent 计划 |
| `TaskSpawner` + `BackgroundTask<T>` | 进程内异步句柄 | 轮询、等待或取消单个 Future |

`TaskSpawner` 明确不是持久任务图 store。重启恢复、依赖关系、claim、重试和
终态结算属于 `RevisionedTaskStore`/`RuntimeDagController` 实现。

## 后台 Future

`BackgroundTask<T>` 是单个已启动 Future 的可克隆句柄，支持非阻塞状态读取、
取消和带可选超时的可重试等待。Clone共享同一个生命周期和取消scope，不复制`T`。
第一个终态waiter消费result；后续waiter会立即得到永久保存的Completed、Failed或
Cancelled disposition，不会等待第二次通知。

```rust,ignore
use echo_agent::tasks::{TaskSpawner, TaskSpawnerConfig};
use std::time::Duration;

let spawner = TaskSpawner::new(TaskSpawnerConfig::default());
let handle = spawner.spawn("fetch-data", async {
    Ok("result".to_string())
});

println!("{:?}", handle.status().await);
let result = handle.wait(Some(Duration::from_secs(30))).await?;
```

进程内生命周期为：

```text
Pending -> Running -> Completed
                   -> Failed
                   -> Cancelled
```

spawner 通过 semaphore 限制并发，并可列出或取消当前进程仍存在的句柄。它不会
序列化 Future 闭包，也不宣称能够在重启后恢复它们。

`default_timeout_secs`是从spawn接纳开始、覆盖容量排队和执行的一次绝对预算。
取消和deadline在排队与运行阶段都会被观察；排队中取消的任务不会启动；
`max_concurrent = 0`产生Failed handle，而不是永久Pending。执行期间取消或超时会先
abort并join child task，再发布terminal status。Child task panic会转为Failed，并可由
`is_panicked()`观察。

每次`wait(timeout)`也只使用该次调用的一次绝对timeout；通知不会重置预算，wait
timeout也不会消费未来的task result。Status、result delivery、list和retention全部读取
同一个process-local terminal state。

## 持久 DAG 执行

需要跨重启恢复的 Agent 工作，应将任务图持久化在单一
`RevisionedTaskStore` 后，并实现窄接口 `RuntimeDagController`。框架执行器
统一负责：

- 完整快照校验和环检测；
- 依赖 ready frontier 计算；
- 有界 Subagent wave；
- attempt 级原子 claim 和 ABA 防护；
- 重试、跳过、暂停、传递阻塞与取消结算；
- 在执行 safe point 重载版本；
- 对非法快照和停滞图 fail closed。

应用在 controller 中负责产品特定的持久化、调度、review 和资源选择。
controller 返回已提交快照，并以 compare-and-set 完成 claim 与结果提交；它不
复制 DAG 主循环。

### 精确 Attempt 控制

每个已 claim 的任务都通过 `TaskClaim::execution_id(run_id, task_id)` 派生
Subagent execution identity。runtime 在容量接纳前预留该 identity，并让同一个
task child cancellation token 贯穿 dispatch、事件、终态结算和 live control。
取消整个 run 会传播到所有 child；`request_attempt_interrupt` 只取消 exact claim，
且在投影控制请求后再次检查持久 claim。

`TeamRuntimeHandle` 绑定同一个 Team run ID、任务 store、`RuntimeTaskService` 和
Subagent control registry。执行期间需要并发检查快照或精确取消时，应保留该 handle：

```rust,ignore
let handle = team.runtime_handle().await?;
let execution = team.execute("review the repository");

let snapshot = handle.snapshot().await?;
let task = snapshot.tasks.iter().find(|task| task.execution.claim.is_some())?;
let claim = task.execution.claim.as_ref()?;
handle
    .request_attempt_interrupt(&task.spec.id, claim)
    .await?;
```

live pending/reserved/active/settled 记录只是进程内投影。
`RuntimeTaskService::reconcile_attempt_control` 在恢复时依据持久 TaskClaim 重建其
有效性。持久 command replay 属于 Host；重放时必须携带 exact claim identity，
不能只依赖 execution name。
如果已 join 的 attempt 无法证明持久终态，等待者会收到类型化 authority error，
而不是无限等待。恢复先向 stale joined waiter 发布持久 supersede 并退休本地
supervisor 状态，再等待尽力而为的 live-controller cleanup；仍 active 的 attempt
继续保留 binding，直到 targeted abort 与 canonical join 完成。
自定义 Team 集成必须实现完整 `TeamDispatchController`，使 dispatch、reservation、
interrupt、cleanup 与 reconciliation 共用一个 live control scope。每个保留 runtime
都有唯一 `handle_id`；同一个业务 run 可持有多个 handle，不会由后创建者覆盖先前控制
权威。active handle 不受历史数量上限淘汰；只有已结算 handle 进入有界的 64 项保留集。
需要并发控制的 caller-supplied runtime 应构造单个
`TeamRuntimeServiceHandle<R>` 并传给 `execute_team_on_runtime_service`，从类型上阻止
graph/output authority 与 execution/CAS authority 来自不同 runtime 实例。

非 Team 的 `RuntimeDagController` adapter 使用
`SubagentExecutor::attempt_control_handle`，把 reservation、dispatch、interrupt 投影、
cleanup 与 reconciliation 绑定到同一个进程 scope。handle 只从
`TaskSubagentContext` 或 `TaskClaim` 派生 identity，不加载任务 store，也不判断 claim
是否 current。公开 command 仍必须进入同一个 `RuntimeTaskService`，由它完成持久
precondition 后再调用 controller 的 live hook。

## 进度

`PhasePlan` 和 `ProgressReporter` 提供任务内结构化进度。`ProgressBridge` 可将
Agent callback 投影为有损 `TaskEventBus` 上的 `TaskEvent::Progress`，供界面
展示。这些事件只是投影，不是任务状态权威；持久状态仍以已提交任务图为准。

## 定时触发

scheduler 模块提供 cron 触发器。定时 callback 可以启动后台 Future 或请求
版本化 run，但 schedule 本身不会创建另一套任务图或执行状态机。

已提交的 scheduler occurrence 复用 framework `DeliveryLedger` 持久记录 claim、
attempt、owner loss 恢复和 terminal settlement。Runner 启动后会在首个周期 tick 前
drain pending FIFO。已接纳但未持久结算的 callback 会先记为 `OutcomeUnknown`，随后以
同一个 occurrence ID 和新的 attempt ID 重投；callback 明确返回错误时形成已知
`Failed` 结算，通用 scheduler 不自动重试业务失败。同一进程中 callback owner 被
drop 或 abort 时，也会在下一次 drain 通过 `OutcomeUnknown` 进入相同恢复路径。

`CronTaskStore` 每次 add 都分配不可变 `definition_id`，status control 则持久递增
`control_revision`。因此 remove 后重新 add exact task clone 仍是新 definition；旧 incarnation
的 queued callback 不能通过 admission，也不能写入 replacement 的 last-run projection。

Callback 会产生外部副作用时，使用
`SchedulerRunner::new_with_occurrence_context` 与 `OccurrenceFireFn`。
`SchedulerInvocation::occurrence_id` 跨 crash replay 保持稳定，应作为幂等 key；
`attempt_id` 只标识一次物理 callback 执行。兼容入口 `FireFn(CronTask)` 仍走同一个
ledger，只是丢弃 durable context。

File-backed task store 使用完整 definition 文件名共置 journal。Store-backed
`CronTaskStore` 在构造 runner 前必须通过 `with_path()` 提供稳定且 backend-specific 的
anchor；Store trait 没有可由 framework 安全猜测的可移植 backend identity。

该合同是 callback at-least-once delivery，不是 external effect exactly-once。副作用
成功后、terminal settlement 前发生 crash 时 callback 可能再次执行。Cron polling
本身也仍是 approximate：runner 离线期间错过的 schedule 不会在没有明确 misfire
policy 时自动补跑。
