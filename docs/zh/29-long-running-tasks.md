# 长程任务

> **状态：已实现。**
> 系统具备完整的长程任务支持，包括非阻塞任务句柄、跨重启恢复、进度追踪、人在回路、定时调度等。

---

## 概述

echo-agent 的长程任务系统由以下子系统组成：

| 子系统 | 模块 | 说明 |
|--------|------|------|
| DAG 任务引擎 | `echo_orchestration::tasks` | 有向无环图任务编排，支持依赖、并行、重试 |
| 后台任务句柄 | `tasks::background_task` | `BackgroundTask<T>` + `TaskSpawner`，非阻塞任务管理 |
| 进度追踪 | `tasks::progress` | `PhasePlan` + `ProgressReporter`，实时进度广播 |
| 人在回路门 | `tasks::human_gate` | `HumanGate`，暂停任务等待人类审批 |
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

---

## 进度追踪（ProgressReporter）

为长程任务提供实时进度反馈：

```
PhasePlan → ProgressReporter → watch::Receiver<TaskProgress>  → SSE/WS/UI
                             → TaskEvent::Progress → TaskEventBus → 日志/持久化
```

### Phase 与 PhasePlan

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

### TaskEvent::Progress

进度事件可注入 `TaskEventBus`，与生命周期事件统一分发：

```rust,ignore
let mut reporter = ProgressReporter::new("task-42".into(), plan);
let bus = TaskEventBus::new();

reporter.enter_phase(0, Some("Searching...".into()));
let progress = reporter.current();
bus.emit(TaskEvent::Progress {
    task_id: "task-42".into(),
    progress,
});
```

完整可运行示例：`cargo run --example demo67_progress`

---

## 人在回路门（HumanGate）

为任务流水线提供人工检查点。运行中的任务可暂停自身，等待人类审批后再继续。

### 核心类型

```rust,ignore
use echo_agent::tasks::{HumanGate, HumanRequest, HumanResponse};

pub struct HumanRequest {
    pub prompt: String,             // 展示给用户的问题
    pub context: serde_json::Value, // 任意上下文
    pub options: Vec<String>,       // 可选响应 ["Approve", "Revise", "Cancel"]
    pub phase: String,              // 等待输入的阶段名
}

pub struct HumanResponse {
    pub selection: String,            // 选择的选项
    pub instructions: Option<String>, // 可选的自由文本指令
}
```

### 使用方式

```rust,ignore
use tokio_util::sync::CancellationToken;

let gate = HumanGate::new();
let cancel = CancellationToken::new();

// 任务侧：发起请求并阻塞等待
let response = gate.request("task-1", HumanRequest {
    prompt: "Review the draft".into(),
    context: serde_json::json!({ "draft": "..." }),
    options: vec!["Approve".into(), "Revise".into(), "Cancel".into()],
    phase: "review".into(),
}, &cancel).await?;

// 前端侧：检查待处理请求并回复
let pending = gate.pending().await;
gate.respond("task-1", HumanResponse {
    selection: "Approve".into(),
    instructions: None,
}).await;
```

| HumanGate 方法 | 说明 |
|----------------|------|
| `request(task_id, req, cancel)` | 阻塞等待回复或取消 |
| `respond(task_id, resp)` | 回复待处理请求 |
| `pending()` | 列出所有待处理请求 |
| `pending_count()` | 待处理数量 |

完整可运行示例：`cargo run --example demo68_human_gate --features tasks,subagent`

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

### CronTaskStore

持久化存储，支持双后端：

| 后端 | 创建方式 |
|------|----------|
| **Store trait**（推荐） | `CronTaskStore::with_store(store)` |
| **文件**（回退） | `CronTaskStore::new()` |

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

| 管理方法 | 说明 |
|----------|------|
| `add_task(task)` | 添加并持久化 |
| `remove_task(id)` | 按 id 前缀删除 |
| `set_status(id, status)` | 启用/禁用 |
| `list_tasks()` | 返回所有任务 |
| `run_once(id)` | 立即触发一次 |
| `reload()` | 从 Store 重新加载 |

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

---

## 集成架构

```
ReactAgent
├── 标准路径: ReAct 循环 (think → act → observe → 重复)
│   └── 可调用 spawn_background_task (非阻塞)
├── Planner 路径: Plan → to_task_dag() → TaskExecutor::execute_all() → final_answer
│   └── 阻塞等待所有 DAG 任务完成（设计如此）
└── BackgroundReviewer: fire-and-forget LLM 调用
```

`execute_all_async()` 作为公共 API 提供，供外部编排器在 ReAct 循环外异步触发 DAG 执行。
