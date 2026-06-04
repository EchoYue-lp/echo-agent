# Agent 运行时与任务系统

> **状态：已实现。**
> 统一的 Agent 运行时 + 可组合的任务子系统，支持执行序列化、DAG 编排、进度追踪、人在回路、定时调度等。

---

## 概述

echo-agent 采用**单一 Agent 引擎**架构：所有执行路径（前台对话、后台任务、子 Agent 调度）共享同一个 `ReactAgent` 实例，通过内置的 `execution_mutex` 保证并发安全。

```
┌─────────────────────────────────────────────────────────────┐
│                   ReactAgent (统一引擎)                      │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ execution_mutex — 全局执行序列化                        │ │
│  │                                                        │ │
│  │  前台 chat ──────┐                                     │ │
│  │  execute()  ─────┤──→ 同一把锁 ──→ 互斥执行            │ │
│  │  chat_stream() ──┤                                     │ │
│  │  后台任务 ───────┘                                     │ │
│  └────────────────────────────────────────────────────────┘ │
│                                                              │
│  可组合能力:                                                 │
│  ├── ReAct 循环 (think → act → observe)                     │
│  ├── 任务规划 (execute_with_planning)                       │
│  ├── 子 Agent 调度 (SubagentExecutor)                       │
│  ├── 自我审查 (ReviewTool)                                  │
│  └── 后台任务 (TaskSpawner / DAG 引擎)                      │
└─────────────────────────────────────────────────────────────┘
```

### 执行序列化

`ReactAgent` 内部持有 `execution_mutex: Arc<tokio::sync::Mutex<()>>`，所有执行入口自动加锁：

| 执行路径 | 加锁位置 | 说明 |
|---------|---------|------|
| `execute()` / `chat()` | `run_react_loop()` 入口 | 非流式执行 |
| `execute_stream()` / `chat_stream()` | `run_stream_channel()` 中 `lock_owned()` | 流式执行，锁移入 spawned task |
| `execute_with_planning()` | 方法入口 | 三阶段规划 |
| `chat_multimodal()` | 方法入口 | 多模态对话 |

这意味着：前台 chat 和后台任务**自动互斥**，调用方无需手动管理任何锁。

### AgentHandle

`AgentHandle` 封装 `Arc<RwLock<ReactAgent>>`，提供安全的读写访问：

```rust,ignore
use echo_agent::prelude::*;

let handle = AgentHandle::new(agent);

// 读访问（可并发，但 execute/chat 内部自动序列化）
let result = handle.read_async(|a| {
    Box::pin(async move { a.execute("task").await })
}).await;

// 写访问（修改配置、注册回调等）
handle.write(|a| { a.add_callback(callback); }).await;
```

---

## 任务子系统

在统一运行时之上，echo-agent 提供以下可组合的任务子系统：

| 子系统 | 模块 | 说明 |
|--------|------|------|
| DAG 任务引擎 | `echo_orchestration::tasks` | 有向无环图任务编排，支持依赖、并行、重试 |
| 后台任务句柄 | `tasks::background_task` | `BackgroundTask<T>` + `TaskSpawner`，非阻塞任务管理 |
| 进度追踪 | `tasks::progress` + `callbacks::ProgressBridge` | `PhasePlan` + `ProgressReporter` + Agent 回调桥接 |
| 人在回路选择 | `human_loop::Selection` | `HumanLoopProvider` 的 `Selection` kind，暂停任务等待人类选择 |
| 复合执行 | `tasks::composite` | `CompositePlan`，异构步骤链的顺序/并行执行 |
| 定时调度 | `scheduler` | `CronTask` + `SchedulerRunner`，基于 cron 表达式的定时触发 |

---

## 后台任务句柄（BackgroundTask）

`BackgroundTask<T>` 提供对异步任务的非阻塞控制：

```rust,ignore
use echo_orchestration::tasks::{TaskSpawner, TaskSpawnerConfig};

let spawner = TaskSpawner::new(TaskSpawnerConfig::default());

// Spawn 后台任务 — 立即返回句柄
let handle = spawner.spawn("fetch-data", async {
    Ok("result".to_string())
});

// 非阻塞状态查询
println!("{:?}", handle.status().await);

// 阻塞等待（带超时）
let result = handle.wait(Some(Duration::from_secs(30))).await?;

// 取消
handle.cancel();
```

### BackgroundTaskStatus 生命周期

```
Pending → Running → Completed
                  ↘ Failed
                  ↘ Cancelled
```

### TaskSpawner

系统级任务管理器，支持并发控制（Semaphore）和跨重启恢复：

```rust,ignore
let spawner = TaskSpawner::new(TaskSpawnerConfig::default())
    .with_store(Arc::new(SqliteTaskStore::new("tasks.db").await?));

// 列出所有任务
let tasks = spawner.list().await;

// 取消指定/全部任务
spawner.cancel("task-id");
spawner.cancel_all();

// 跨重启恢复
let incomplete = spawner.resume_from_store().await?;
```

### Per-task 执行逻辑

每个 `Task` 可设置独立的 `execute_fn`，覆盖 executor 的全局函数：

```rust,ignore
let task = Task::new("code-review", "Review pull request")
    .with_execute_fn(Arc::new(|ctx| Box::pin(async move {
        Ok(format!("Reviewed: {}", ctx.description))
    })));
```

### 非阻塞 DAG 执行

`TaskExecutor::execute_all_async()` 立即返回句柄，不阻塞调用方：

```rust,ignore
let handles = executor.execute_all_async();
// Agent 可以继续做其他工作
for handle in &handles {
    if !handle.is_completed().await {
        println!("Task {} still running", handle.name);
    }
}
```

### 跨重启恢复

```rust,ignore
let executor = TaskExecutor::new(manager, config)
    .with_task_store(store)
    .with_execute_fn(my_execute_fn);  // execute_fn 不可序列化，需重新注册

let results = executor.resume_from_store().await?;
```

---

## Agent 后台任务工具

`tasks` feature 下，以下工具随 `enable_task` 自动注册：

| 工具 | 描述 |
|------|------|
| `spawn_background_task` | Spawn 一个后台任务，返回 task ID |
| `check_task_status` | 查询后台任务的当前状态 |
| `list_background_tasks` | 列出所有活跃的后台任务 |

这些工具在 ReAct 循环中被 Agent 调用时，会创建后台任务并立即返回 task ID，不阻塞 Agent 的推理循环。

---

## 进度追踪

### ProgressBridge — Agent 回调桥接

`ProgressBridge` 将 `AgentCallback` 事件翻译为 `TaskEvent::Progress`，实现执行过程中的实时进度反馈：

```
AgentCallback (on_iteration, on_tool_start, ...)
    ↓ ProgressBridge
TaskEvent::Progress → TaskEventBus → 前端 / 日志
```

当 `max_iterations` 已知时，进度按线性计算。未知时使用递减曲线，渐近逼近 95%，确保任务不会在 `on_final_answer` 之前报告"完成"。

```rust,ignore
use echo_agent::agent::callbacks::ProgressBridge;

let bridge = Arc::new(ProgressBridge::new(
    task_id.clone(),
    event_bus.clone(),
    0,  // 0 = 无限迭代，使用递减曲线
));

// 注册为 Agent 回调
agent.write(|a| { a.add_callback(bridge.clone()); }).await;

// 执行任务（ReactAgent 内部自动序列化）
let result = agent.read_async(|a| {
    Box::pin(async move { a.execute(&prompt).await })
}).await;

// 清理
bridge.disable();
agent.write(|a| { a.remove_callbacks_by_type_name("ProgressBridge"); }).await;
```

### PhasePlan — 结构化进度

`Phase` 定义流水线中的单个阶段，支持权重、重试、超时和人工检查点：

```rust,ignore
use echo_agent::tasks::{Phase, PhasePlan};

let plan = PhasePlan::new(vec![
    Phase::new("search",  "Search",  2.0),  // 权重 2
    Phase::new("analyze", "Analyze", 3.0),  // 权重 3
    Phase::new("report",  "Report",  1.0),  // 权重 1
]);

plan.progress_pct(0, 0.5);  //  16.7%  (1.0 / 6.0)
plan.progress_pct(1, 0.0);  //  33.3%  (2.0 / 6.0)
plan.progress_pct(2, 1.0);  // 100.0%  (6.0 / 6.0)
```

### ProgressReporter

基于 `watch` 通道的进度广播器，最新值语义：

| 方法 | 说明 |
|------|------|
| `new(task_id, plan)` | 创建 reporter |
| `enter_phase(idx, msg)` | 进入新阶段 |
| `update_phase_progress(pct, msg)` | 阶段内进度更新（0.0–1.0） |
| `subscribe()` | 获取 `watch::Receiver<TaskProgress>` |
| `current()` | 获取当前快照 |

完整可运行示例：`cargo run --example demo67_progress`

---

## 人在回路选择（Selection Checkpoint）

为任务流水线提供人工检查点。运行中的任务可通过 `HumanLoopProvider` 的 `Selection` kind 暂停自身，等待人类选择后再继续。这与工具审批（Approval）和文本输入（Input）共用同一套基础设施。

### 核心类型

```rust,ignore
use echo_agent::human_loop::{HumanLoopProvider, HumanLoopRequest, HumanLoopResponse};

// 构造选择请求
let request = HumanLoopRequest::selection(
    "task-1",                                      // 任务 ID
    "Review the draft and choose an action",        // 提示
    vec!["Approve".into(), "Revise".into(), "Cancel".into()], // 选项
)
.with_context(serde_json::json!({ "draft": "..." }))
.with_phase("review");
```

### 使用方式

```rust,ignore
// 通过 HumanLoopProvider 请求选择（与审批/输入共用入口）
let response = provider.request(request).await?;

match response {
    HumanLoopResponse::Selection { selection, instructions } => {
        if selection == "Cancel" {
            return Err("Task cancelled by user".into());
        }
        if let Some(inst) = instructions {
            // 处理用户的自由文本指令
            phase_state.insert("human_feedback".into(), Value::String(inst));
        }
    }
    _ => { /* 处理其他响应类型 */ }
}
```

### 与 LongRunningTaskRunner 集成

```rust,ignore
let runner = LongRunningTaskRunner::new(task_id, plan, store, cancel)
    .with_human_loop_provider(provider);  // 接入 HumanLoopProvider
```

完整可运行示例：`cargo run --example demo68_human_gate --features tasks,subagent,human-loop`

---

## 定时调度（Scheduler）

基于 cron 表达式的定时任务能力，支持持久化存储和运行时管理。

### CronTask

```rust,ignore
use echo_agent::scheduler::{CronTask, CronTaskStatus};

let task = CronTask::new("daily-backup", "0 2 * * *", "Run nightly backup");
task.validate_cron();      // -> true
task.next_run();           // -> Some(DateTime<Utc>)
```

Cron 表达式为 5 字段标准格式：`分 时 日 月 星期`

| 示例 | 含义 |
|------|------|
| `0 2 * * *` | 每天凌晨 2:00 |
| `*/5 * * * *` | 每 5 分钟 |
| `0 9 * * 1` | 每周一 9:00 |

### SchedulerRunner

后台调度器，每 30 秒 tick 一次，到期时触发任务：

```rust,ignore
use echo_agent::scheduler::{CronTask, CronTaskStore, SchedulerRunner, FireFn};

let store = CronTaskStore::new();
store.add(CronTask::new("daily-backup", "0 2 * * *", "Run nightly backup"))?;

let fire_fn: FireFn = Arc::new(|task| Box::pin(async move {
    Ok(format!("Executed: {}", task.name))
}));

let runner = Arc::new(SchedulerRunner::new(store, cancel, fire_fn));
runner.clone().spawn();               // 启动后台 tick 循环
runner.run_once("daily").await?;      // 手动触发一次
runner.set_status("daily", CronTaskStatus::Disabled).await?;
```

完整可运行示例：`cargo run --example demo70_scheduler`

---

## 类型化元数据

`Task` 支持附加任意类型数据，`metadata_json` 跨重启存活：

```rust,ignore
use echo_agent::tasks::Task;
use serde::Serialize;

#[derive(Serialize)]
struct ResearchParams { topic: String, max_papers: u32 }

let task = Task::new("r1", "Research task")
    .with_metadata(ResearchParams { topic: "AI".into(), max_papers: 20 });

// 类型化访问
let params = task.get_metadata::<ResearchParams>().unwrap();
```
