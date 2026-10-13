# 版本化任务图

## 概览

echo-agent 将单任务、Todo 风格列表和依赖 DAG 统一表示为一个版本化任务图，
不存在另一套任务管理状态机。

- `TaskRevisionService` 是唯一的 CRUD、关系、校验和版本权威。
- `RuntimeTaskService` 是唯一公开的依赖执行入口。
- `TaskSpawner` 只追踪进程内后台 Future，不拥有持久任务关系。
- Plan 是任务图之上的可编辑、版本化 artifact，不进入审批状态机。

Task graph 是动态 Task 依赖与精确 claim/attempt 结算的权威。调用方定义的
`Graph` 或 `DagWorkflow` 可以在某个 Task 内执行，但其节点和输出不会提交 Task
状态。边界与组合规则见 [ADR 0059](../adr/0059-task-workflow-dag-authority.md)。

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

应用拥有具体 extension schema 时，直接使用 typed extension helper。framework 仍将其
保存为 JSON，但序列化错误和 task identity 会由同一个公开 API 统一处理：

```rust,no_run
use echo_agent::tasks::TaskSpec;
use serde::{Deserialize, Serialize};
use std::error::Error;

#[derive(Clone, Serialize, Deserialize)]
struct ApplicationTaskData {
    role: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let spec = TaskSpec {
        id: "inspect".into(),
        title: "检查仓库".into(),
        description: "读取相关文件".into(),
        depends_on: Vec::new(),
        max_retries: 1,
        extension: serde_json::Value::Null,
    }
    .with_extension(ApplicationTaskData { role: "explorer".into() })?;
    let _data: ApplicationTaskData = spec.extension_as()?;
    Ok(())
}
```

`TaskDraft` 与 `TaskSpecPatch` 也提供相同 helper。`extension_as` 对 patch 返回
`Option<T>`，调用方可以区分“未提供 extension”和“明确提供了 extension”。

共享生命周期包含 `Pending`、`Running`、`Blocked`、`Retrying`、`Paused`、
`Completed`、`Failed`、`TimedOut`、`Skipped` 和 `Cancelled`，所有迁移由
`TaskStatus::transition_to` 校验。

## 单一 CRUD 服务

默认框架 Agent 注册三个任务工具：

| 工具 | 契约 |
|------|------|
| `task_create` | 原子创建完整任务图，或携带 `base_revision` 追加任务 |
| `task_update` | 对规格、关系、顺序、跳过或状态应用一次乐观并发 patch |
| `task_list` | 读取当前已提交任务图的有界分页；支持 `limit`（1–100）、opaque `cursor` 和 `detail_level`（`summary`/`full`） |

首次 `task_create` 必须在一个 `tasks` 数组中携带所有相关任务。后续修改携带
当前 `base_revision`；过时写入返回版本冲突，不会覆盖更新的状态。

`task_list` 默认返回 20 个任务的 summary 页。结果 metadata 会在仍有后续任务时
提供 `page.next_cursor`、`page.returned`、`page.total` 和 `page.truncated`。后续请求
必须使用相同的已提交任务图和 limit 携带 opaque cursor；查询或快照变化会使 cursor
失效。`detail_level=full` 只额外返回依赖、重试计数和非空生命周期 detail，不建立
第二个 store 或 reducer。

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

`RuntimeTaskService<C>` 通过薄 `RuntimeDagController` adapter 反复加载已提交的
`RuntimePlanSnapshot`：

1. 校验完整快照并检测环。
2. 根据已提交依赖和状态计算 ready frontier。
3. 使用版本、attempt、规格 hash 和唯一 claim id 原子领取任务。
4. 分派有界、无冲突的 Subagent wave。
5. 每个 attempt join 后立即通过 compare-and-set 结算，不等待同 wave 的其它活跃 sibling。
6. 在 wave 边界结算 typed 取消或可恢复暂停 receipt。
7. 在下一 safe point 重新加载，使新版本生效。

controller 是持久化、Subagent 调度、review 和产品资源策略的薄应用适配器，
不得重新实现 ready-frontier 主循环或第二套依赖状态机。已 join attempt 的 review、
CAS 与 cleanup future 与 dispatch join、root cancellation grace 并行受监督，慢 review
或 Store 不得阻塞其它精确控制；未知提交结果的 CAS 保留 owner，不为取消期限直接丢弃。
下一 wave 仍等待所有 settlement receipt。

服务统一处理传递失败阻塞、跳过与暂停、有界重试、取消结算、过时 claim 和
停滞检测。dispatch resolution 保留 `Failed` 与 `TimedOut` 两种不同终态；requeue
request 会声明 retry budget 耗尽时应提交哪一种终态，framework 通过同一 exact-claim
compare-and-set 路径提交，持久化 adapter 不得在事后重新解释。依赖失败只形成 typed
`DagDependencyState` 投影，不会持久化为
`TaskStatus::Blocked`；重试失败祖先后，派生阻塞会自动消失。`Blocked` 仍可表达
review、缺少输入等显式产品策略。暂停会清除 claim，恢复到 Pending 时不消耗 retry。
Skipped 依赖以 typed waiver 传给 Subagent，不会伪造依赖输出。attempt 级 claim
identity 可防止旧 dispatch 覆盖重新领取后的结果。

### 同一 Store 的执行与结果恢复

CRUD 与运行时均进入同一个 `TaskRevisionService` 注册的 Store。可执行 backend
实现 `RevisionedTaskStore::compare_and_commit_execution`：原子比较 relation revision
和完整 execution map，保持任务规格、顺序及 context 不变，在相同关系版本提交执行
事实。backend 通过无副作用的 `supports_runtime_execution()` 声明执行支持，默认 false；
可执行 backend 显式 opt in。仅支持 CRUD 的 backend 在 runtime load/mutation 前明确
返回 `Unsupported`，不会旁路到另一张内存图。

有输出合同的任务使用 `RuntimeTaskResolutionRequest::CompletedWithResult`；
`TaskExecution.result` 与 Completed 在同一事务提交，图恢复时依赖结果也随之恢复。
缺失的 `None` 与显式提供的 JSON null 不同。adapter 必须做 typed decode，对缺失或
不合法结果返回明确错误，不能制造成功数据；Team 结果读取直接使用这张图。

单 claim 控制先通过 `TaskRevisionService::request_runtime_task_interruption` 持久化
Cancelled/Paused 请求，再调用 `RuntimeTaskService::request_attempt_interrupt` 投影
到运行中的精确 attempt。过时请求返回 false，不能控制新 attempt；请求与迟到结果
由同一 execution CAS 排序。局部任务停止只约束本节点和后代，其它 ready root 与
后续 wave 继续运行；明确的 run cancellation token 才触发全 run 结算。Cancelled 的优先级高于 Paused：
run 暂停结算必须保留已接受的单任务取消终态，checkpoint 恢复和 resume 不得复活它。

取舍与 backend 合同见 [ADR 0060](../adr/0060-task-execution-store-cas-and-local-interruption.md)。

## 投影与进度

`TaskEvent` 和 `TaskProgress` 是面向消费者的投影。应用可从已提交 extension
派生 Todo、证据和 UI 数据；下一步运行时决策始终基于通过单一 service/controller
边界加载的已提交版本。

公开 service 构造见 `tests/facade_smoke.rs`，确定性 controller 行为见私有
`runtime_executor` 测试。

`RuntimeTaskService::execute` 的调用方 future 被 drop 后，同一个 framework driver
仍持有 claim/reservation、物理 dispatch join 和 CAS 结算。调用方退出只取消传入 token
的 child scope；结果必须来自规范结算或明确的未知权威。后端已提交 claim 却在返回
identity 前 unwind 时保留 Running，等待后端 owner/fencing 或权威收据恢复，不猜测
重试或终态。见 [ADR 0060](../adr/0060-task-execution-store-cas-and-local-interruption.md)。
