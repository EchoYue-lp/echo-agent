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
取消和带可选超时的可重试等待。

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

## 进度

`PhasePlan` 和 `ProgressReporter` 提供任务内结构化进度。`ProgressBridge` 可将
Agent callback 投影为有损 `TaskEventBus` 上的 `TaskEvent::Progress`，供界面
展示。这些事件只是投影，不是任务状态权威；持久状态仍以已提交任务图为准。

## 定时触发

scheduler 模块提供 cron 触发器。定时 callback 可以启动后台 Future 或请求
版本化 run，但 schedule 本身不会创建另一套任务图或执行状态机。
