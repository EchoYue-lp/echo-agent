# Long-Running Tasks

> **Status: Implemented.**
> The system provides full long-running task support including non-blocking handles, cross-restart recovery, progress tracking, human-in-the-loop gates, and cron scheduling.

---

## Overview

The echo-agent long-running task system consists of the following subsystems:

| Subsystem | Module | Description |
|-----------|--------|-------------|
| DAG Task Engine | `echo_orchestration::tasks` | Directed acyclic graph task orchestration with dependencies, parallelism, and retry |
| Background Task Handle | `tasks::background_task` | `BackgroundTask<T>` + `TaskSpawner` for non-blocking task management |
| Progress Tracking | `tasks::progress` | `PhasePlan` + `ProgressReporter` for real-time progress broadcasting |
| Human Gate | `tasks::human_gate` | `HumanGate` to pause tasks awaiting human approval |
| Composite Execution | `tasks::composite` | `CompositePlan` for sequential/parallel execution of heterogeneous step chains |
| Task Scheduler | `scheduler` | `CronTask` + `SchedulerRunner` for cron-based periodic triggering |

---

## Background Task Handle (BackgroundTask)

`BackgroundTask<T>` provides non-blocking control over async tasks:

```rust,ignore
use echo_orchestration::tasks::{TaskSpawner, TaskSpawnerConfig};

let spawner = TaskSpawner::new(TaskSpawnerConfig::default());

// Spawn a background task — returns handle immediately
let handle = spawner.spawn("fetch-data", async {
    Ok("result".to_string())
});

// Non-blocking status check
println!("{:?}", handle.status().await);

// Blocking wait with timeout
let result = handle.wait(Some(Duration::from_secs(30))).await?;

// Cancel
handle.cancel();
```

### BackgroundTaskStatus Lifecycle

```
Pending → Running → Completed
                  ↘ Failed
                  ↘ Cancelled
```

### TaskSpawner

System-level task manager with concurrency control (Semaphore) and cross-restart recovery:

```rust,ignore
let spawner = TaskSpawner::new(TaskSpawnerConfig::default())
    .with_store(Arc::new(SqliteTaskStore::new("tasks.db").await?));

// List all tasks
let tasks = spawner.list().await;

// Cancel specific/all tasks
spawner.cancel("task-id");
spawner.cancel_all();

// Cross-restart recovery
let incomplete = spawner.resume_from_store().await?;
```

### Per-task Execution Logic

Each `Task` can set its own `execute_fn`, overriding the executor's global function:

```rust,ignore
let task = Task::new("code-review", "Review pull request")
    .with_execute_fn(Arc::new(|ctx| Box::pin(async move {
        Ok(format!("Reviewed: {}", ctx.description))
    })));
```

### Non-blocking DAG Execution

`TaskExecutor::execute_all_async()` returns handles immediately without blocking the caller:

```rust,ignore
let handles = executor.execute_all_async();
// Agent can continue doing other work
for handle in &handles {
    if !handle.is_completed().await {
        println!("Task {} still running", handle.name);
    }
}
```

### Cross-Restart Recovery

```rust,ignore
let executor = TaskExecutor::new(manager, config)
    .with_task_store(store)
    .with_execute_fn(my_execute_fn);  // execute_fn is not serializable, must re-register

let results = executor.resume_from_store().await?;
```

---

## Agent Background Task Tools

With the `tasks` feature, the following tools are auto-registered when `enable_task` is true:

| Tool | Description |
|------|-------------|
| `spawn_background_task` | Spawn a background task, return task ID |
| `check_task_status` | Query the current status of a background task |
| `list_background_tasks` | List all active background tasks |

---

## Progress Tracking (ProgressReporter)

Provides real-time progress feedback for long-running tasks:

```
PhasePlan → ProgressReporter → watch::Receiver<TaskProgress>  → SSE/WS/UI
                             → TaskEvent::Progress → TaskEventBus → logging/persistence
```

### Phase and PhasePlan

`Phase` defines a single stage in a pipeline, with weight, retry, timeout, and human checkpoint support:

```rust,ignore
use echo_agent::tasks::{Phase, PhasePlan};

let plan = PhasePlan::new(vec![
    Phase::new("search",  "Search",  2.0),  // weight 2
    Phase::new("analyze", "Analyze", 3.0),  // weight 3
    Phase::new("report",  "Report",  1.0),  // weight 1
]);

plan.progress_pct(0, 0.5);  //  16.7%  (1.0 / 6.0)
plan.progress_pct(1, 0.0);  //  33.3%  (2.0 / 6.0)
plan.progress_pct(2, 1.0);  // 100.0%  (6.0 / 6.0)
```

### ProgressReporter

Watch-channel-based progress broadcaster with latest-value semantics:

| Method | Description |
|--------|-------------|
| `new(task_id, plan)` | Create a reporter |
| `enter_phase(idx, msg)` | Enter a new phase |
| `update_phase_progress(pct, msg)` | Intra-phase progress update (0.0–1.0) |
| `subscribe()` | Get a `watch::Receiver<TaskProgress>` |
| `current()` | Get current snapshot |

### TaskEvent::Progress

Progress events can be emitted into `TaskEventBus`, unified with lifecycle events:

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

Runnable example: `cargo run --example demo67_progress`

---

## Human Gate (HumanGate)

Provides human-in-the-loop checkpoints for task pipelines. Running tasks can pause themselves and wait for human approval before continuing.

### Core Types

```rust,ignore
use echo_agent::tasks::{HumanGate, HumanRequest, HumanResponse};

pub struct HumanRequest {
    pub prompt: String,             // Question shown to the user
    pub context: serde_json::Value, // Arbitrary context
    pub options: Vec<String>,       // Available responses ["Approve", "Revise", "Cancel"]
    pub phase: String,              // Name of the phase waiting for input
}

pub struct HumanResponse {
    pub selection: String,            // Selected option
    pub instructions: Option<String>, // Optional free-text instructions
}
```

### Usage

```rust,ignore
use tokio_util::sync::CancellationToken;

let gate = HumanGate::new();
let cancel = CancellationToken::new();

// Task side: send request and block
let response = gate.request("task-1", HumanRequest {
    prompt: "Review the draft".into(),
    context: serde_json::json!({ "draft": "..." }),
    options: vec!["Approve".into(), "Revise".into(), "Cancel".into()],
    phase: "review".into(),
}, &cancel).await?;

// Frontend side: check pending requests and respond
let pending = gate.pending().await;
gate.respond("task-1", HumanResponse {
    selection: "Approve".into(),
    instructions: None,
}).await;
```

| HumanGate Method | Description |
|------------------|-------------|
| `request(task_id, req, cancel)` | Block until response or cancellation |
| `respond(task_id, resp)` | Respond to a pending request |
| `pending()` | List all pending requests |
| `pending_count()` | Number of pending requests |

Runnable example: `cargo run --example demo68_human_gate --features tasks,subagent`

---

## Task Scheduler

Cron-based task scheduling with persistence and runtime management.

### CronTask

```rust,ignore
use echo_agent::scheduler::{CronTask, CronTaskStatus};

let task = CronTask::new("daily-backup", "0 2 * * *", "Run nightly backup");
task.validate_cron();      // -> true
task.next_run();           // -> Some(DateTime<Utc>)
```

Cron expressions use the standard 5-field format: `min hour dom month dow`

| Example | Meaning |
|---------|---------|
| `0 2 * * *` | Daily at 2:00 AM |
| `*/5 * * * *` | Every 5 minutes |
| `0 9 * * 1` | Every Monday at 9:00 AM |

### CronTaskStore

Persistent storage with dual backend:

| Backend | Creation |
|---------|----------|
| **Store trait** (recommended) | `CronTaskStore::with_store(store)` |
| **File** (fallback) | `CronTaskStore::new()` |

### SchedulerRunner

Background scheduler with 30-second tick interval, fires tasks when due:

```rust,ignore
use echo_agent::scheduler::{CronTask, CronTaskStore, SchedulerRunner, FireFn};

let store = CronTaskStore::new();
store.add(CronTask::new("daily-backup", "0 2 * * *", "Run nightly backup"))?;

let fire_fn: FireFn = Arc::new(|task| Box::pin(async move {
    Ok(format!("Executed: {}", task.name))
}));

let runner = Arc::new(SchedulerRunner::new(store, cancel, fire_fn));
runner.clone().spawn();               // Start background tick loop
runner.run_once("daily").await?;      // Manual trigger
runner.set_status("daily", CronTaskStatus::Disabled).await?;
```

| Management Method | Description |
|-------------------|-------------|
| `add_task(task)` | Add and persist |
| `remove_task(id)` | Remove by id prefix |
| `set_status(id, status)` | Enable/disable |
| `list_tasks()` | Return all tasks |
| `run_once(id)` | Trigger immediately |
| `reload()` | Reload from store |

Runnable example: `cargo run --example demo70_scheduler`

---

## Typed Metadata

`Task` supports attaching arbitrary typed data. `metadata_json` survives cross-restart serialization:

```rust,ignore
use echo_agent::tasks::Task;
use serde::Serialize;

#[derive(Serialize)]
struct ResearchParams { topic: String, max_papers: u32 }

let task = Task::new("r1", "Research task")
    .with_metadata(ResearchParams { topic: "AI".into(), max_papers: 20 });

// Typed access
let params = task.get_metadata::<ResearchParams>().unwrap();
```

---

## Integration Architecture

```
ReactAgent
├── Standard path: ReAct loop (think → act → observe → repeat)
│   └── Can call spawn_background_task (non-blocking)
├── Planner path: Plan → to_task_dag() → TaskExecutor::execute_all() → final_answer
│   └── Blocks until all DAG tasks complete (by design)
└── BackgroundReviewer: fire-and-forget LLM call
```

`execute_all_async()` is available as a public API for external orchestrators that need to trigger DAG execution asynchronously outside the ReAct loop.
