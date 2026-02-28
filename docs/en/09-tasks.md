# DAG Task Planning System

## What It Is

The DAG task planning system allows an Agent to decompose a complex goal into sub-tasks with dependencies, forming a Directed Acyclic Graph (DAG). Tasks are then executed in topological order — tasks with no interdependencies can run in parallel, while dependent tasks are sequenced.

This is the core mechanism behind `AgentRole::Planner`.

---

## Problem It Solves

Simple ReAct loops struggle with complex multi-step tasks:
- **Implicit planning**: The LLM makes ad-hoc decisions at each step, lacking a global view
- **No result reuse**: Cannot easily carry intermediate results between steps
- **Serial bottleneck**: Parallelizable sub-tasks are forced to run sequentially
- **Opaque state**: Task progress is hard to track or visualize

DAG task planning follows a "think first, then act" approach: the LLM first decomposes the goal into a structured task graph, then executes it efficiently following dependency order.

---

## Core Concepts

### Task Node

```rust
Task {
    id: String,                   // unique identifier
    description: String,          // what to do
    status: TaskStatus,           // Pending / Running / Completed / Failed / Skipped
    dependencies: Vec<String>,    // IDs of prerequisite tasks
    priority: u8,                 // priority (1-10, higher = more urgent)
    result: Option<String>,       // execution result
    reasoning: Option<String>,    // notes or rationale
}
```

### Task State Machine

```
Pending → Running → Completed
                 ↘ Failed
                 ↘ Skipped
```

### TaskManager

Provides:
- `add_task()` — add a task node
- `detect_circular_dependencies()` — detect cycles
- `get_topological_order()` — topological sort (Kahn's algorithm)
- `get_ready_tasks()` — tasks whose dependencies are all completed
- `get_next_task()` — highest-priority ready task
- `update_task()` — update task status
- `visualize_dependencies()` — output a Mermaid diagram

---

## Usage (Planner Mode)

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new(
    "gpt-4o",
    "planner",
    "You are a task planning expert. For complex tasks:
     1. Use the plan tool to declare your planning intent
     2. Use create_task for each sub-task with proper dependencies
     3. Use update_task to mark tasks complete and record results
     4. Use final_answer to summarize when all tasks are done"
)
.role(AgentRole::Planner)
.enable_tool(true)
.enable_task(true);

let mut agent = ReactAgent::new(config);
agent.add_tool(Box::new(WebSearchTool));
agent.add_tool(Box::new(CalculatorTool));

let answer = agent.execute(
    "Research and compare Rust vs Go concurrency performance, then give a recommendation"
).await?;
```

The LLM's planning process (fully observable):
```
[think] Need to search Rust concurrency info, Go concurrency info, then compare
[create_task] id="search_rust"  description="Search Rust concurrency benchmarks"    deps=[]
[create_task] id="search_go"    description="Search Go concurrency benchmarks"      deps=[]
[create_task] id="compare"      description="Analyze and compare the findings"       deps=["search_rust","search_go"]
[create_task] id="recommend"    description="Produce a selection recommendation"     deps=["compare"]
```

---

## Direct TaskManager API

```rust
use echo_agent::tasks::{Task, TaskManager, TaskStatus};

let mut mgr = TaskManager::default();

// Build a DAG: task3 depends on task1 and task2
mgr.add_task(Task { id: "task1".into(), description: "Fetch raw data".into(),
    status: TaskStatus::Pending, dependencies: vec![], priority: 8, ..Default::default() });
mgr.add_task(Task { id: "task2".into(), description: "Clean data".into(),
    status: TaskStatus::Pending, dependencies: vec!["task1".into()], priority: 7, ..Default::default() });
mgr.add_task(Task { id: "task3".into(), description: "Analyze data".into(),
    status: TaskStatus::Pending, dependencies: vec!["task2".into()], priority: 9, ..Default::default() });

// Cycle detection
if mgr.has_circular_dependencies() {
    eprintln!("Circular dependency detected!");
}

// Topological order
let order = mgr.get_topological_order()?;
println!("Execution order: {:?}", order); // ["task1", "task2", "task3"]

// Get currently executable tasks
let ready = mgr.get_ready_tasks();
println!("Ready: {}", ready[0].id); // "task1"

// Mark complete
mgr.update_task("task1", TaskStatus::Completed);
let ready = mgr.get_ready_tasks();
println!("Next ready: {}", ready[0].id); // "task2"

// Mermaid visualization
println!("{}", mgr.visualize_dependencies());
```

Mermaid output:
```
graph TD
    task1["Fetch raw data"]
    task2["Clean data"]
    task3["Analyze data"]
    task2 --> task1
    task3 --> task2
```

---

## Built-in Task Tools

When `enable_task(true)` is set, these tools are automatically registered for the LLM to call:

| Tool | Purpose |
|------|---------|
| `plan` | Declare planning intent (triggers Planner mode) |
| `create_task` | Create a sub-task with dependencies |
| `update_task` | Update task status and result |
| `list_tasks` | List all tasks and their statuses |
| `get_execution_order` | Get topological execution order |
| `visualize_dependencies` | Output a Mermaid dependency graph |

See: `examples/demo02_tasks.rs`
