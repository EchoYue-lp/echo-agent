# Agent Runtime & Task System

> **Status: Implemented.**
> Unified Agent runtime + composable task subsystems with execution serialization, DAG orchestration, progress tracking, human-in-the-loop, and cron scheduling.

---

## Overview

echo-agent uses a **single Agent engine** architecture: all execution paths (foreground chat, background tasks, subagent dispatch) share the same `ReactAgent` instance, with a built-in `execution_mutex` ensuring concurrency safety.

```
┌─────────────────────────────────────────────────────────────┐
│                ReactAgent (Unified Engine)                   │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ execution_mutex — Global Execution Serialization       │ │
│  │                                                        │ │
│  │  Foreground chat ──┐                                   │ │
│  │  execute()  ───────┤──→ Same mutex ──→ Mutual exclusion│ │
│  │  chat_stream() ────┤                                   │ │
│  │  Background tasks ─┘                                   │ │
│  └────────────────────────────────────────────────────────┘ │
│                                                              │
│  Composable capabilities:                                    │
│  ├── ReAct loop (think → act → observe)                     │
│  ├── Revisioned task graph tools                            │
│  ├── Subagent dispatch (SubagentExecutor)                   │
│  ├── Self-review (ReviewTool)                               │
│  └── Background tasks (TaskSpawner / DAG engine)            │
└─────────────────────────────────────────────────────────────┘
```

### Execution Serialization

`ReactAgent` holds an internal `execution_mutex: Arc<tokio::sync::Mutex<()>>`, with all execution entry points automatically acquiring the lock:

| Execution Path | Lock Location | Description |
|---------------|---------------|-------------|
| `execute()` / `chat()` | `run_react_loop()` entry | Non-streaming execution |
| `execute_stream()` / `chat_stream()` | `run_stream_channel()` via `lock_owned()` | Streaming execution, lock moved into spawned task |
| `task_create/task_update/task_list` | Tool calls | Revisioned task graph CRUD |
| `chat_multimodal()` | Method entry | Multimodal conversation |

This means: foreground chat and background tasks **automatically serialize** — callers don't need to manually manage any locks.

### AgentHandle

`AgentHandle` wraps `Arc<RwLock<ReactAgent>>` for safe read/write access:

```rust,ignore
use echo_agent::prelude::*;

let handle = AgentHandle::new(agent);

// Read access (concurrent reads allowed, but execute/chat internally serialize)
let result = handle.read_async(|a| {
    Box::pin(async move { a.execute("task").await })
}).await;

// Write access (modify config, register callbacks, etc.)
handle.write(|a| { a.add_callback(callback); }).await;
```

---

## Task Subsystems

Built on top of the unified runtime, echo-agent provides the following composable task subsystems:

| Subsystem | Module | Description |
|-----------|--------|-------------|
| DAG Task Engine | `echo_agent::tasks` | Directed acyclic graph task orchestration with dependencies, parallelism, and retry |
| Background Task Handle | `tasks::background_task` | `BackgroundTask<T>` + `TaskSpawner` for non-blocking task management |
| Progress Tracking | `tasks::progress` + `callbacks::ProgressBridge` | `PhasePlan` + `ProgressReporter` + Agent callback bridging |
| Human Selection | `human_loop::Selection` | `HumanLoopProvider` Selection kind to pause tasks awaiting human choice |
| Composite Execution | `tasks::composite` | `CompositePlan` for sequential/parallel execution of heterogeneous step chains |
| Task Scheduler | `scheduler` | `CronTask` + `SchedulerRunner` for cron-based periodic triggering |

---

## Background Task Handle (BackgroundTask)

`BackgroundTask<T>` provides non-blocking control over async tasks:

```rust,ignore
use echo_agent::tasks::{TaskSpawner, TaskSpawnerConfig};

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

When called by the Agent in the ReAct loop, these tools create background tasks and return the task ID immediately without blocking the Agent's reasoning loop.

---

## Progress Tracking

### ProgressBridge — Agent Callback Bridging

`ProgressBridge` translates `AgentCallback` events into `TaskEvent::Progress`, enabling real-time progress feedback during execution:

```
AgentCallback (on_iteration, on_tool_start, ...)
    ↓ ProgressBridge
TaskEvent::Progress → TaskEventBus → Frontend / Logging
```

When `max_iterations` is known, progress is calculated linearly. When unknown, a diminishing curve asymptotically approaches 95%, ensuring the task never reports "complete" before `on_final_answer` fires.

```rust,ignore
use echo_agent::agent::callbacks::ProgressBridge;

let bridge = Arc::new(ProgressBridge::new(
    task_id.clone(),
    event_bus.clone(),
    0,  // 0 = unlimited iterations, use diminishing curve
));

// Register as Agent callback
agent.write(|a| { a.add_callback(bridge.clone()); }).await;

// Execute task (ReactAgent internally serializes)
let result = agent.read_async(|a| {
    Box::pin(async move { a.execute(&prompt).await })
}).await;

// Cleanup
bridge.disable();
agent.write(|a| { a.remove_callbacks_by_type_name("ProgressBridge"); }).await;
```

### PhasePlan — Structured Progress

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

Runnable example: `cargo run --example demo67_progress`

---

## Human Selection Checkpoint

Provides human-in-the-loop checkpoints for task pipelines. Running tasks can pause via `HumanLoopProvider`'s `Selection` kind and wait for human choice before continuing. This shares the same infrastructure as tool approval (Approval) and text input (Input).

### Core Types

```rust,ignore
use echo_agent::human_loop::{HumanLoopProvider, HumanLoopRequest, HumanLoopResponse};

// Construct a selection request
let request = HumanLoopRequest::selection(
    "task-1",                                      // task ID
    "Review the draft and choose an action",        // prompt
    vec!["Approve".into(), "Revise".into(), "Cancel".into()], // options
)
.with_context(serde_json::json!({ "draft": "..." }))
.with_phase("review");
```

### Usage

```rust,ignore
// Request selection via HumanLoopProvider (shared with approval/input)
let response = provider.request(request).await?;

match response {
    HumanLoopResponse::Selection { selection, instructions } => {
        if selection == "Cancel" {
            return Err("Task cancelled by user".into());
        }
        if let Some(inst) = instructions {
            // Handle free-text instructions from the user
            phase_state.insert("human_feedback".into(), Value::String(inst));
        }
    }
    _ => { /* handle other response types */ }
}
```

### Integration with LongRunningTaskRunner

```rust,ignore
let runner = LongRunningTaskRunner::new(task_id, plan, store, cancel)
    .with_human_loop_provider(provider);  // connect HumanLoopProvider
```

Runnable example: `cargo run --example demo68_human_gate --features tasks,subagent,human-loop`

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
