# 长程任务

> **状态：分析与设计建议。**
> 当前系统尚无通用的长程任务机制。
> 本文档分析现有能力、缺失项及推荐设计。

---

## 现有能力

### 1. DAG 任务系统（`echo-orchestration::tasks`，feature = `tasks`）

DAG 任务系统将复杂目标分解为带依赖的子任务，按拓扑序执行。无依赖的子任务通过 `tokio::spawn` 并行执行，受 `Semaphore` 限流。

**生命周期受限于父 Agent 会话：**
```
用户请求 → Agent 创建 Task → TaskExecutor::execute_all() 阻塞等待 → 返回最终答案
```

关键特征：
- `execute_all()` 阻塞调用方 Agent 循环，直到所有任务到达终态
- 任务生命周期绑定在单次 `agent.execute()` 调用内
- SQLite 持久化（`SqliteTaskStore`、`SqliteCheckpointStore`）仅用于 **会话内崩溃恢复**，不支持跨进程重启后继续
- 所有任务共用同一个 `TaskExecuteFn` —— 不同任务类型无法有不同的执行逻辑

### 2. BackgroundReviewer（`improve` feature）

每轮对话结束后，`BackgroundReviewer::review()` 通过 `tokio::spawn` fork 出一个后台任务，回顾对话并决定是否更新记忆或技能。

关键特征：
- Fire-and-forget：spawn 出的任务独立于主循环运行
- 但 `review()` 内部执行 `tokio::spawn(...).await`，**实际阻塞了调用方** —— 等同于同步
- 外部代码无法获取 handle 来 poll 状态或取消
- 无队列、无背压、无去重

---

## 缺失的能力

通用**长程任务**系统需要：

| 能力 | 现状 |
|------|------|
| 提交任务 → 获取句柄 | 无 |
| 轮询任务状态 | `TaskManager::get_task()` 存在但仅限 session 内 |
| 取消运行中的任务 | `TaskExecutor::cancel_task()` 存在但仅内存级 |
| 任务完成后获取结果 | `Task::result` 存在但 session 结束后丢失 |
| 进程重启后继续执行 | SQLite 持久化已存在，但缺少 resume API |
| 不同任务类型不同执行逻辑 | `TaskExecuteFn` 是全局单函数 |
| 单任务超时 | 已支持（`Task::timeout_secs`） |
| 指数退避重试 | 已支持（`TaskExecutor` 内） |
| 并发控制 | Semaphore 限流，可配置 |
| 进度/状态事件 | `TaskEventBus` 广播通道 |

---

## 割裂分析

### DAG Task vs ReAct 循环

当 Planner 角色介入时，Agent 切换到 DAG 驱动模式，与标准 ReAct 循环**互斥**：

```
ReactAgent
├── 标准路径: ReAct 循环 (think → act → observe → 重复)
└── Planner 路径: Plan → to_task_dag() → TaskExecutor::execute_all() → final_answer
```

两条路径不共享状态（除了消息历史）。DAG 任务无法在执行中途"让出"控制权回到 ReAct 循环，ReAct 循环也无法 spawn 一个后台 DAG 后继续运行。

### DAG Task vs BackgroundReviewer

两套完全独立的系统，无共享抽象：
- DAG Task：结构化分解、依赖图、并行执行、重试
- BackgroundReviewer：单次 fire-and-forget LLM 调用，用于每轮对话后的分析

### DAG Task vs Handoff

`HandoffManager` 在 Agent 之间转移控制权，但 Handoff 目标是完整的 Agent 会话 —— 不与 DAG 任务调度器集成。无法只 handoff 单个子任务，只能 handoff 整个对话。

### 架构图

```
┌─────────────────────────────────────────────────────────┐
│                    Agent Session                         │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────┐  │
│  │  ReAct 循环   │  │  PlanExecute │  │  Background   │  │
│  │  (默认)      │  │  (DAG tasks) │  │  Review       │  │
│  │              │  │              │  │  (fire+forget)│  │
│  │  think→act   │  │  plan→exec   │  │               │  │
│  │  →observe    │  │  →全阻塞     │  │  spawn+await  │  │
│  └──────────────┘  └──────────────┘  └───────────────┘  │
│         │                 │                  │           │
│         └─────────┬───────┘                  │           │
│                   │                          │           │
│              互斥关系                   与两者均无集成       │
│         (Planner 角色切换)                              │
└─────────────────────────────────────────────────────────┘
```

---

## 推荐设计

### 核心抽象：`BackgroundTask`

```rust
/// 后台运行中或已完成的任务的句柄。
pub struct BackgroundTask<T> {
    /// 唯一任务 ID（跨重启可持久化）
    pub id: String,
    /// 当前状态
    status: Arc<RwLock<BackgroundTaskStatus>>,
    /// 最终结果的 oneshot 接收端
    result_rx: Mutex<Option<oneshot::Receiver<Result<T>>>>,
    /// 取消令牌
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
    /// 非阻塞查询当前状态。
    pub fn status(&self) -> BackgroundTaskStatus { ... }

    /// 等待完成（可设超时）。
    pub async fn wait(self, timeout: Option<Duration>) -> Result<T> { ... }

    /// 请求取消。
    pub fn cancel(&self) { ... }
}
```

### TaskSpawner

```rust
/// 在系统层面 spawn 并管理后台任务。
pub struct TaskSpawner {
    tasks: Arc<DashMap<String, Arc<dyn AnyBackgroundTask>>>,
    store: Option<Arc<dyn TaskStore>>,       // 跨重启持久化
    max_concurrent: usize,
    semaphore: Arc<Semaphore>,
}

impl TaskSpawner {
    /// 将闭包作为后台任务 spawn，立即返回句柄。
    pub fn spawn<F, T>(&self, name: &str, fut: F) -> BackgroundTask<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    { ... }

    /// 将 Agent 执行作为后台任务 spawn。
    pub fn spawn_agent(
        &self,
        agent: Arc<dyn Agent>,
        input: String,
    ) -> BackgroundTask<String> { ... }

    /// 列出所有任务（用于状态面板）。
    pub fn list(&self) -> Vec<TaskSummary> { ... }

    /// 重启后恢复 pending/in-progress 任务。
    pub async fn resume_from_store(&self) -> Result<Vec<BackgroundTask<String>>> { ... }
}
```

### 集成点

1. **ReAct 循环**：Agent 可调用 `spawn_background_task` 工具来异步 offload 工作，不阻塞主循环
2. **PlanExecute**：不再 `execute_all()` 阻塞，改为 `execute_all_async()` 返回 `BackgroundTask<Vec<TaskExecutionResult>>`
3. **Handoff**：Handoff 目标可通过 `TaskSpawner::spawn_agent()` 作为后台任务执行
4. **BackgroundReviewer**：重构为使用 `TaskSpawner` 替代裸 `tokio::spawn`

### 优先级建议

| 优先级 | 事项 | 工作量 |
|--------|------|--------|
| P0 | 修复 `BackgroundReviewer::review()` —— 去掉 `.await`，返回句柄 | 小 |
| P1 | 添加 `BackgroundTask<T>` 句柄抽象（poll/wait/cancel） | 中 |
| P2 | `TaskExecuteFn` 从全局改为 per-task（`Task` 增加 `execute_fn` 字段） | 中 |
| P3 | `TaskExecutor` 增加 `execute_all_async()` 返回 `BackgroundTask` | 中 |
| P4 | 通过 `SqliteTaskStore` 实现跨重启任务恢复 | 大 |
| P5 | Agent 增加 `spawn_background_task` 工具 | 中 |

---

## 总结

- DAG 任务系统是一个**并行子任务执行器**，不是长程任务系统
- BackgroundReviewer 是 **fire-and-forget 但阻塞调用方**（因为 `.await`）
- 两套系统彼此完全隔离，与 ReAct 循环也隔离
- 架构分层良好，基础组件（持久化、重试、取消、Semaphore）扎实 —— 缺失的是将它们串联成真正长程任务系统的 **handle/poll/resume 抽象**
