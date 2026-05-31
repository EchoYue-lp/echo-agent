# Long-Running Tasks

> **Status: Analysis & Design Recommendation.**
> The current system does NOT have a general-purpose long-running task mechanism.
> This document analyzes what exists, what's missing, and the recommended design.

---

## What Exists Today

### 1. DAG Task System (`echo-orchestration::tasks`, feature `tasks`)

The DAG task system decomposes a complex goal into sub-tasks with dependencies, then executes them in topological order. Independent tasks run in parallel via `tokio::spawn`, bounded by a `Semaphore`.

**Lifecycle is bounded by the parent agent session:**
```
User request → Agent creates Tasks → TaskExecutor::execute_all() blocks → Final answer
```

Key characteristics:
- `execute_all()` blocks the calling agent loop until all tasks reach a terminal state
- Tasks are ephemeral — they live and die within one `agent.execute()` call
- SQLite persistence (`SqliteTaskStore`, `SqliteCheckpointStore`) exists but is only used for **crash recovery within a session**, not cross-restart resumption
- All tasks share a single `TaskExecuteFn` — the same execution logic applies to every task

### 2. Background Review (`improve` feature)

After each conversation turn, `BackgroundReviewer::review()` spawns a `tokio::spawn` that replays the conversation and decides whether to update memory or skills.

Key characteristics:
- Fire-and-forget: the spawned task runs independently of the main loop
- BUT `review()` does `tokio::spawn(...).await`, so it **blocks the caller** — effectively synchronous
- No handle/token is returned for polling or cancellation by external code
- No queue, no backpressure, no deduplication

---

## What's Missing

A general-purpose **long-running task** system needs:

| Capability | Current State |
|---|---|
| Submit task → get a handle | None |
| Poll task status | `TaskManager::get_task()` exists but session-bound |
| Cancel a running task | `TaskExecutor::cancel_task()` exists but in-memory only |
| Retrieve results after completion | `Task::result` exists but lost after session ends |
| Survive process restart | SQLite persistence exists but no resume API |
| Different execution logic per task type | `TaskExecuteFn` is global (one function for all tasks) |
| Timeout per task | Supported (`Task::timeout_secs`) |
| Retry with backoff | Supported in `TaskExecutor` |
| Concurrency control | Semaphore-based, configurable |
| Progress/status events | `TaskEventBus` with broadcast channel |

---

## Isolation Analysis

### DAG Tasks vs ReAct Loop

When the Planner role engages, the agent enters a DAG-driven mode that is **mutually exclusive** with the standard ReAct loop:

```
ReactAgent
├── Standard path: ReAct loop (think → act → observe → repeat)
└── Planner path: Plan → to_task_dag() → TaskExecutor::execute_all() → final_answer
```

The two paths do not share state beyond the agent's message history. There is no mechanism for a DAG task to "yield" back to the ReAct loop mid-execution, nor for the ReAct loop to spawn a background DAG and continue.

### DAG Tasks vs BackgroundReviewer

These are entirely separate systems with no shared abstraction:
- DAG tasks: structured decomposition, dependency graph, parallel execution, retry
- BackgroundReviewer: single fire-and-forget LLM call for post-turn analysis

### DAG Tasks vs Handoff

`HandoffManager` transfers control from one agent to another, but the handoff target runs as a full agent session — it does not integrate with the DAG task scheduler. You cannot hand off a single sub-task; you hand off the entire conversation.

### Diagram

```
┌─────────────────────────────────────────────────────────┐
│                    Agent Session                         │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────┐  │
│  │  ReAct Loop  │  │  PlanExecute │  │  Background   │  │
│  │  (default)   │  │  (DAG tasks) │  │  Review       │  │
│  │              │  │              │  │  (fire+forget)│  │
│  │  think→act   │  │  plan→exec   │  │               │  │
│  │  →observe    │  │  →all block  │  │  spawn+await  │  │
│  └──────────────┘  └──────────────┘  └───────────────┘  │
│         │                 │                  │           │
│         └─────────┬───────┘                  │           │
│                   │                          │           │
│            Mutually exclusive         No integration      │
│         (Planner role switches)       with either path    │
└─────────────────────────────────────────────────────────┘
```

---

## Recommended Design

### Core Abstraction: `BackgroundTask`

```rust
/// A handle to a running or completed background task.
pub struct BackgroundTask<T> {
    /// Unique task ID (survives restarts)
    pub id: String,
    /// Current status
    status: Arc<RwLock<BackgroundTaskStatus>>,
    /// Oneshot receiver for the final result
    result_rx: Mutex<Option<oneshot::Receiver<Result<T>>>>,
    /// Cancellation token
    cancel: CancellationToken,
}

pub enum BackgroundTaskStatus {
    Pending,
    Running { started_at: Instant },
    Completed { finished_at: Instant },
    Failed { error: String, at: Instant },
    Cancelled,
}

impl<T: Send + 'static> BackgroundTask<T> {
    /// Poll current status without blocking.
    pub fn status(&self) -> BackgroundTaskStatus { ... }

    /// Wait for completion (with optional timeout).
    pub async fn wait(self, timeout: Option<Duration>) -> Result<T> { ... }

    /// Request cancellation.
    pub fn cancel(&self) { ... }
}
```

### Task Spawner

```rust
/// Spawns and manages background tasks across the system.
pub struct TaskSpawner {
    tasks: Arc<DashMap<String, Arc<dyn AnyBackgroundTask>>>,
    store: Option<Arc<dyn TaskStore>>,       // for cross-restart persistence
    max_concurrent: usize,
    semaphore: Arc<Semaphore>,
}

impl TaskSpawner {
    /// Spawn a closure as a background task. Returns a handle immediately.
    pub fn spawn<F, T>(&self, name: &str, fut: F) -> BackgroundTask<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    { ... }

    /// Spawn an agent execution as a background task.
    pub fn spawn_agent(
        &self,
        agent: Arc<dyn Agent>,
        input: String,
    ) -> BackgroundTask<String> { ... }

    /// List all tasks (for status dashboards).
    pub fn list(&self) -> Vec<TaskSummary> { ... }

    /// Resume pending/in-progress tasks after a restart.
    pub async fn resume_from_store(&self) -> Result<Vec<BackgroundTask<String>>> { ... }
}
```

### Integration Points

1. **ReAct loop**: Agent can call a `spawn_background_task` tool to offload work without blocking
2. **PlanExecute**: Instead of `execute_all()` blocking, `execute_all_async()` returns a `BackgroundTask<Vec<TaskExecutionResult>>`
3. **Handoff**: Handoff targets can be spawned as background tasks via `TaskSpawner::spawn_agent()`
4. **BackgroundReviewer**: Refactored to use `TaskSpawner` instead of raw `tokio::spawn`

### Priority: What to Fix First

| Priority | Item | Effort |
|----------|------|--------|
| P0 | Fix `BackgroundReviewer::review()` — remove `.await` on the spawned task, return a handle | Small |
| P1 | Add `BackgroundTask<T>` handle abstraction with poll/wait/cancel | Medium |
| P2 | Make `TaskExecuteFn` per-task instead of global (add `execute_fn` field to `Task`) | Medium |
| P3 | Add `execute_all_async()` to `TaskExecutor` that returns a `BackgroundTask` | Medium |
| P4 | Add cross-restart task resumption via `SqliteTaskStore` | Large |
| P5 | Add `spawn_background_task` tool for the agent to offload work | Medium |

---

## Summary

- The DAG task system is a **parallel sub-task executor**, not a long-running task system
- The BackgroundReviewer is **fire-and-forget but blocks the caller** due to `.await`
- The two systems are completely isolated from each other and from the ReAct loop
- The architecture is well-layered and the building blocks (persistence, retry, cancellation, semaphore) are solid — what's missing is the **handle/poll/resume abstraction** that ties them together into a true long-running task system
