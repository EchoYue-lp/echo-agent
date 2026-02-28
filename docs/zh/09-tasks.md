# DAG 任务规划系统（Tasks）

## 是什么

DAG 任务规划系统允许 Agent 将一个复杂目标分解为多个有依赖关系的子任务，形成有向无环图（Directed Acyclic Graph），然后按拓扑顺序逐步执行——没有依赖关系的任务可以并行，有依赖的按序执行。

这是 `AgentRole::Planner` 的核心机制。

---

## 解决什么问题

对于复杂的多步骤任务，简单的 ReAct 循环存在问题：

- **隐式规划**：LLM 每一步都在"临时决策"，缺乏全局视角
- **重复劳动**：无法在步骤间复用已有结果
- **串行瓶颈**：可以并行的子任务被强制串行执行
- **状态不透明**：任务进度难以追踪和可视化

DAG 任务规划先"想清楚"再"做"：LLM 先将目标分解为结构化的任务图，再按依赖关系高效执行。

---

## 核心概念

### Task（任务节点）

```rust
Task {
    id: String,            // 唯一标识
    description: String,   // 任务描述
    status: TaskStatus,    // Pending / Running / Completed / Failed / Skipped
    dependencies: Vec<String>, // 前置任务 ID 列表
    priority: u8,          // 优先级（1-10，数字越大优先级越高）
    result: Option<String>,// 执行结果
    reasoning: Option<String>, // 执行理由或备注
}
```

### TaskStatus（状态机）

```
Pending → Running → Completed
                 ↘ Failed
                 ↘ Skipped
```

### TaskManager（DAG 管理器）

提供：
- `add_task()` — 添加任务节点
- `detect_circular_dependencies()` — 检测循环依赖
- `get_topological_order()` — 拓扑排序（Kahn 算法）
- `get_ready_tasks()` — 获取当前可执行的任务（依赖全部完成）
- `get_next_task()` — 按优先级取下一个任务
- `update_task()` — 更新任务状态
- `visualize_dependencies()` — 输出 Mermaid 图表

---

## 使用方式（Planner 模式）

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new(
    "qwen3-max",
    "planner",
    "你是一个任务规划专家。收到复杂任务时：
     1. 先使用 plan 工具声明你的规划意图
     2. 用 create_task 创建每个子任务，并设置依赖关系
     3. 用 update_task 标记任务完成并记录结果
     4. 所有任务完成后用 final_answer 汇总"
)
.role(AgentRole::Planner)   // Planner 角色自动启用任务工具
.enable_tool(true)
.enable_task(true);

let mut agent = ReactAgent::new(config);
// 注册实际执行需要的工具
agent.add_tool(Box::new(WebSearchTool));
agent.add_tool(Box::new(CalculatorTool));

let answer = agent.execute(
    "研究一下 Rust 和 Go 在并发性能上的对比，并给出选型建议"
).await?;
```

LLM 的规划过程（透明可观测）：
```
[think] 需要搜索 Rust 并发资料、Go 并发资料，然后分析比较
[create_task] id="search_rust"  description="搜索 Rust 并发性能资料"  deps=[]
[create_task] id="search_go"    description="搜索 Go 并发性能资料"    deps=[]
[create_task] id="compare"      description="对比分析两者差异"         deps=["search_rust","search_go"]
[create_task] id="recommend"    description="给出选型建议"             deps=["compare"]
```

---

## 直接使用 TaskManager API

```rust
use echo_agent::tasks::{Task, TaskManager, TaskStatus};

let mut mgr = TaskManager::default();

// 构建 DAG：task3 依赖 task1 和 task2
mgr.add_task(Task { id: "task1".into(), description: "获取原始数据".into(),
    status: TaskStatus::Pending, dependencies: vec![], priority: 8, .. Default::default() });
mgr.add_task(Task { id: "task2".into(), description: "清洗数据".into(),
    status: TaskStatus::Pending, dependencies: vec!["task1".into()], priority: 7, .. Default::default() });
mgr.add_task(Task { id: "task3".into(), description: "分析数据".into(),
    status: TaskStatus::Pending, dependencies: vec!["task2".into()], priority: 9, .. Default::default() });

// 循环依赖检测
if mgr.has_circular_dependencies() {
    eprintln!("存在循环依赖！");
}

// 拓扑顺序
let order = mgr.get_topological_order()?;
println!("执行顺序: {:?}", order); // ["task1", "task2", "task3"]

// 获取当前可执行任务（依赖均已完成）
let ready = mgr.get_ready_tasks();
println!("可执行任务: {}", ready[0].id); // "task1"

// 标记完成
mgr.update_task("task1", TaskStatus::Completed);
let ready = mgr.get_ready_tasks();
println!("下一个可执行: {}", ready[0].id); // "task2"

// Mermaid 可视化
println!("{}", mgr.visualize_dependencies());
```

Mermaid 输出示例：
```
graph TD
    task1["获取原始数据"]
    task2["清洗数据"]
    task3["分析数据"]
    task2 --> task1
    task3 --> task2
```

---

## 任务系统内置工具

启用 `enable_task(true)` 时，自动注册以下工具供 LLM 调用：

| 工具名 | 功能 |
|--------|------|
| `plan` | 声明规划意图（触发 Planner 模式） |
| `create_task` | 创建带依赖的子任务 |
| `update_task` | 更新任务状态和结果 |
| `list_tasks` | 列出所有任务及其状态 |
| `get_execution_order` | 获取拓扑排序执行顺序 |
| `visualize_dependencies` | 输出 Mermaid 依赖图 |

对应示例：`examples/demo02_tasks.rs`
