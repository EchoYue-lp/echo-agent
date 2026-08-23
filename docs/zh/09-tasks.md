# 版本化任务图

## 概览

echo-agent 将单任务、Todo 风格列表和依赖 DAG 统一表示为一个版本化任务图，
不存在另一套任务管理状态机。

- `TaskRevisionService` 是唯一的 CRUD、关系、校验和版本权威。
- `RuntimeDagExecutor` 是唯一的依赖执行内核。
- `TaskSpawner` 只追踪进程内后台 Future，不拥有持久任务关系。
- Plan 是任务图之上的可编辑、版本化 artifact，不进入审批状态机。

## 任务模型

每个已提交节点将不可变规格与可变执行状态分开：

```rust
pub struct Task {
    pub spec: TaskSpec,
    pub execution: TaskExecution,
}
```

`TaskSpec` 只包含任务 ID、标题、描述、依赖、重试上限和 opaque 产品 extension。
`TaskExecution` 包含状态、重试计数、失败指纹以及可选的 attempt 级 claim。
coding 类型、Subagent 选择、文件、工具、checks、review 和 UI 投影都由应用层
typed extension 持有。

共享生命周期包含 `Pending`、`Running`、`Blocked`、`Retrying`、`Paused`、
`Completed`、`Failed`、`TimedOut`、`Skipped` 和 `Cancelled`，所有迁移由
`TaskStatus::transition_to` 校验。

## 单一 CRUD 服务

默认框架 Agent 注册三个任务工具：

| 工具 | 契约 |
|------|------|
| `task_create` | 原子创建完整任务图，或携带 `base_revision` 追加任务 |
| `task_update` | 对规格、关系、顺序、跳过或状态应用一次乐观并发 patch |
| `task_list` | 读取当前已提交任务图版本 |

首次 `task_create` 必须在一个 `tasks` 数组中携带所有相关任务。后续修改携带
当前 `base_revision`；过时写入返回版本冲突，不会覆盖更新的状态。

需要持久化或产品策略的应用注入自己的 `RevisionedTaskStore` 和
`TaskToolPolicy`：

```rust,ignore
use echo_agent::tasks::{
    DefaultTaskToolPolicy, InMemoryRevisionedTaskStore, TaskRevisionService,
};
use std::sync::Arc;

let service = Arc::new(TaskRevisionService::new(
    Arc::new(InMemoryRevisionedTaskStore::new()),
    Arc::new(DefaultTaskToolPolicy::new("run-42")),
));

let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .task_revision_service(service)
    .build()?;
```

策略适配器可以解析 scope 并附加无损产品 extension，但通用 patch 语义和 DAG 校验
始终由框架负责。

## 运行时执行

`RuntimeDagExecutor<C>` 通过 `RuntimeDagController` 反复加载已提交的
`RuntimePlanSnapshot`：

1. 校验完整快照并检测环。
2. 根据已提交依赖和状态计算 ready frontier。
3. 使用版本、attempt、规格 hash 和唯一 claim id 原子领取任务。
4. 分派有界、无冲突的 Subagent wave。
5. 以 compare-and-set 语义完成或放弃每个 claim。
6. 在下一 safe point 重新加载，使新版本生效。

controller 是持久化、Subagent 调度、review 和产品资源策略的薄应用适配器，
不得重新实现 ready-frontier 主循环或第二套依赖状态机。

执行器统一处理传递失败阻塞、跳过与暂停、有界重试、取消结算、过时 claim 和
停滞检测。attempt 级 claim identity 可防止旧 dispatch 覆盖重新领取后的结果。

## 投影与进度

`TaskEvent` 和 `TaskProgress` 是面向消费者的投影。应用可从已提交 extension
派生 Todo、证据和 UI 数据；下一步运行时决策始终基于通过单一 service/controller
边界加载的已提交版本。

直接服务用法见 `demo48_personal_assistant`，确定性 controller 用法见
`runtime_executor` 测试。
