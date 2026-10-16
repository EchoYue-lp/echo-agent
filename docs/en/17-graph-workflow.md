# Graph Workflow — LangGraph-style Agent Orchestration

## What It Is

Graph Workflow models Agent execution as a **directed graph with shared state**, supporting:
- Linear pipelines
- Conditional branching
- Loops
- Parallel fan-out/fan-in

This is echo-agent's implementation of the LangGraph pattern.

---

## Core Concepts

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Graph Workflow Structure                          │
│                                                                      │
│   SharedState: KV store shared across nodes + structured message    │
│                                                                      │
│   ┌─────────┐     ┌─────────┐     ┌─────────┐                      │
│   │ Node A  │────▶│ Node B  │────▶│ Node C  │                      │
│   │ (Agent) │     │(Function)│    │ (Agent) │                      │
│   └─────────┘     └─────────┘     └─────────┘                      │
│        │                                                │            │
│        │           ┌─────────┐                         │            │
│        └──────────▶│ Node D  │◀────────────────────────┘            │
│                    │(Condition)│    (conditional edge)               │
│                    └─────────┘                                      │
│                         │                                           │
│                    ┌────┴────┐                                     │
│                    ▼         ▼                                     │
│               ┌────────┐ ┌────────┐                                │
│               │Node E  │ │Node F  │  (parallel fan-out)            │
│               └────────┘ └────────┘                                │
│                    │         │                                     │
│                    └────┬────┘                                     │
│                         ▼                                           │
│                    ┌────────┐                                       │
│                    │  END   │  (fan-in)                            │
│                    └────────┘                                       │
└─────────────────────────────────────────────────────────────────────┘
```

---

## SharedState

State shared across all nodes:

```rust
use echo_agent::workflow::SharedState;

let state = SharedState::new();
state.set("input", "Hello");
state.set("count", 42);

let input: String = state.get("input").unwrap_or_default();
let count: i32 = state.get("count").unwrap_or(0);

// Access messages
for msg in state.messages() {
    println!("[{}] {}", msg.role, msg.content);
}
```

---

## GraphBuilder DSL

```rust
use echo_agent::workflow::{GraphBuilder, SharedState, shared_agent};

// Create agents
let researcher = shared_agent(
    ReactAgentBuilder::simple("qwen3-max", "Researcher")?
);
let writer = shared_agent(
    ReactAgentBuilder::simple("qwen3-max", "Writer")?
);

let graph = GraphBuilder::new("research_pipeline")
    // Agent nodes with input/output key mapping
    .add_agent_node("researcher", researcher.clone())
        .input_key("task")
        .output_key("research")
    .add_agent_node("writer", writer.clone())
        .input_key("research")
        .output_key("result")
    // Function nodes for data transformation
    .add_function_node("format", |state| Box::pin(async move {
        let result: String = state.get("result").unwrap_or_default();
        state.set("final", format!("### Report\n\n{}", result));
        Ok(())
    }))
    // Graph structure
    .set_entry("researcher")
    .add_edge("researcher", "writer")
    .add_edge("writer", "format")
    .set_finish("format")
    .build()?;

// Execute
let state = SharedState::new();
state.set("task", "Research Rust async patterns");
let result = graph.run(state).await?;
println!("{}", result.state.get::<String>("final").unwrap_or_default());
```

---

## Conditional Edges

```rust
let graph = GraphBuilder::new("conditional_flow")
    .add_agent_node("analyzer", analyzer)
    .add_agent_node("detailed", detailed_agent)
    .add_agent_node("summary", summary_agent)
    // Conditional edge based on state
    .add_conditional_edge("analyzer", |state| {
        let complexity: f64 = state.get("complexity").unwrap_or(0.5);
        if complexity > 0.7 { "detailed" } else { "summary" }
    })
    .add_edge("detailed", "__end__")
    .add_edge("summary", "__end__")
    .set_entry("analyzer")
    .set_finish("__end__")
    .build()?;
```

---

## Parallel Fan-out/Fan-in

```rust
let graph = GraphBuilder::new("parallel_processing")
    .add_function_node("input", |state| Box::pin(async move {
        state.set("data", vec!["a", "b", "c"]);
        Ok(())
    }))
    .add_function_node("process_a", |state| Box::pin(async move {
        let data: Vec<String> = state.get("data").unwrap_or_default();
        state.set("result_a", format!("A: {:?}", data));
        Ok(())
    }))
    .add_function_node("process_b", |state| Box::pin(async move {
        let data: Vec<String> = state.get("data").unwrap_or_default();
        state.set("result_b", format!("B: {:?}", data));
        Ok(())
    }))
    .add_function_node("merge", |state| Box::pin(async move {
        let a: String = state.get("result_a").unwrap_or_default();
        let b: String = state.get("result_b").unwrap_or_default();
        state.set("final", format!("{}\n{}", a, b));
        Ok(())
    }))
    // Fan-out: input → process_a, process_b (parallel)
    .set_entry("input")
    .add_edge("input", "process_a")
    .add_edge("input", "process_b")
    // Fan-in: process_a, process_b → merge
    .add_edge("process_a", "merge")
    .add_edge("process_b", "merge")
    .set_finish("merge")
    .build()?;
```

---

## Streaming Events

```rust
let mut stream = graph.run_stream(state).await?;

while let Some(event) = stream.next().await {
    match event? {
        WorkflowEvent::NodeStart { node_name, step_index } => {
            println!("▶ Starting node: {} (step {})", node_name, step_index);
        }
        WorkflowEvent::NodeEnd { node_name, step_index, elapsed } => {
            println!("✓ Completed: {} in {:?}", node_name, elapsed);
        }
        WorkflowEvent::Token { node_name, token } => {
            print!("{}", token);  // Stream tokens from agent nodes
        }
        WorkflowEvent::NodeError { node_name, error } => {
            eprintln!("✗ Error in {}: {}", node_name, error);
        }
        WorkflowEvent::Completed { result, total_steps, elapsed } => {
            println!("\n=== Workflow completed in {:?} ===", elapsed);
            println!("Result: {}", result);
        }
    }
}
```

`NodeError` is a terminal diagnostic event: the stream emits it before ending
with the same error and never emits `Completed` after a failed node. When a
parallel branch fails, the workflow cancels and drains its remaining siblings
so a hung branch cannot mask the failure or outlive caller cancellation.
Agent nodes forward `AgentEvent::Token` as `WorkflowEvent::Token`; their final
answer is committed to `SharedState` before `NodeEnd` is emitted.

`run`, `run_until_interrupt`, checkpoint resume, and `run_stream` all delegate
node routing, path/step accounting, fan-out, errors, and completion to one
internal execution loop. Streaming is an event projection of that loop, not a
second executor.

---

## Declarative YAML Workflow

Define workflows without writing Rust code:

```yaml
# workflow.yaml
name: content_pipeline
nodes:
  - name: researcher
    type: agent
    model: qwen3-max
    system_prompt: "You are a research assistant"
    input_key: task
    output_key: research
    
  - name: writer
    type: agent
    model: qwen3-max
    system_prompt: "You are a writer"
    input_key: research
    output_key: draft
    
  - name: reviewer
    type: agent
    model: qwen3-max
    system_prompt: "You are an editor"
    input_key: draft
    output_key: final

edges:
  - from: researcher
    to: writer
  - from: writer
    to: reviewer

entry: researcher
finish: [reviewer]
```

```rust
use echo_agent::workflow::Graph;

let graph = Graph::from_yaml("workflow.yaml")?;
let state = SharedState::new();
state.set("task", "Write about Rust ownership");
let result = graph.run(state).await?;
```

---

## Workflow Types

| Type | Description | Use Case |
|------|-------------|----------|
| `Graph` | LangGraph-style with conditional edges, loops | Complex multi-agent orchestration |
| `SequentialWorkflow` | Simple pipeline, step N output → step N+1 input | ETL pipelines |
| `ConcurrentWorkflow` | All agents run in parallel, results merged | Parallel analysis |
| `DagWorkflow` | Fixed acyclic Agent pipeline; predecessor text output feeds successor nodes | Static pipeline invocation |

These are public Workflow APIs, separate from the revisioned Task graph.
`Graph` owns conditional routing, shared state and checkpoint continuation;
`DagWorkflow` owns one fixed topological pipeline invocation and has no Task
claim or checkpoint contract. Use `RuntimeTaskService` for a dynamic Task DAG.
If a Workflow is dispatched by a Task, its output is evidence for the Task's
exact claim settlement, not another Task status authority. See
[ADR 0059](../adr/0059-task-workflow-dag-authority.md).

### SequentialWorkflow

```rust
use echo_agent::workflow::{SequentialWorkflow, SequentialWorkflowBuilder, WorkflowStep};

let workflow = SequentialWorkflowBuilder::new()
    .add_step(WorkflowStep::agent("step1", agent1))
    .add_step(WorkflowStep::agent("step2", agent2))
    .add_step(WorkflowStep::function("format", |input| Box::pin(async move {
        Ok(format!("Result: {}", input))
    })))
    .build()?;

let result = workflow.run("Initial input").await?;
```

### ConcurrentWorkflow

```rust
use echo_agent::workflow::{ConcurrentWorkflow, ConcurrentWorkflowBuilder};

let workflow = ConcurrentWorkflowBuilder::new()
    .add_agent("analyzer1", agent1)
    .add_agent("analyzer2", agent2)
    .add_agent("analyzer3", agent3)
    .build()?;

let result = workflow.run("Analyze this data").await?;
// All agents run in parallel, results merged
```

---

## Checkpoint & Resume

A Graph binds its own `Arc<dyn CheckpointStore>` through `with_checkpoint_store`,
using `MemoryCheckpointStore` or `FileCheckpointStore`. Call
`run_until_interrupt(state)` with configured before/after interrupts, obtain
`InterruptState.checkpoint` from `RunUntilInterruptResult::Interrupted`, then call
`resume(checkpoint, ApprovalDecision::Approved)`. Use `resume_with_state` for state
updates and `branch_from` for a branch. The executable
[demo39 contract](https://github.com/EchoYue-lp/echo-agent/blob/main/echo-agent-learning/tests/example_contracts/demo39_workflow.rs)
shows basic Graph/state usage; the checkpoint entry points are in the current
[Graph source](https://github.com/EchoYue-lp/echo-agent/blob/main/echo-orchestration/src/workflow/graph.rs).

Executable Approved/Modified resumes require the bound Store to declare
`supports_protected_claims()` and successfully perform
`protect_claim(checkpoint_id, attempt_id)` after winning the original claim and
before any Node effect. The default capability is false: unsupported execution
fails before claim, and `branch_from` fails before saving a fork. Deferred still
only loads/validates; Rejected uses the original claim/ack settlement without
executing a Node. Fresh `run`, `run_until_interrupt` and `run_stream` are not gated
on protection. Custom and SDK-backed Stores must implement atomic protection on
the same claim authority, plus the existing renew, ack, requeue and generation-CAS
operations. Declaring capability is not itself evidence of durability.

Only actual original continuation closure allows a success to ack or a known
ordinary failure to follow its existing requeue policy. A failed or lost protect
acknowledgement starts no Node and does not ack/requeue an uncertain protection.
`ExecutionNotSettled` is observation knowledge, not Failed or retry permission;
without an authoritative later source the original claim stays held. A genuine
later terminal from the same bound source followed by its actual EOF can confirm
that source's observation. EOF alone, disconnection or missing local ownership
cannot. A local Node terminal cannot clear a different Store's observation.

Graph still renews the exact attempt through heartbeat, but a protected claim
cannot expire or replay automatically after missed renewals. Genuine old
unprotected records retain their lease recovery policy. Cancellation, node
timeout, heartbeat failure, fanout sibling failure and stream closure signal
cancellation and continue draining the same started source. Dropping a borrowed
waiter or a polled stream does not execute the Node again. Retain the original
Graph/Agent/Store parent after an unknown observation; destroying every parent
is not physical-stop proof or an automatic-recovery guarantee.

Memory protection is limited to the same Store instance. File retains it across
restart through the original directory OS lock and versioned private records.
Expiry, save/delete/clear cannot implicitly remove protection; mixed clear
refuses the whole mutation before deleting any record. The verified old automatic reader
rejects the protected format, but old delete/clear remain unsafe: do not downgrade
writers onto protected data. There is no new force-unlock or receipt-query API.
See [ADR 0088](../adr/0088-subagent-settlement-observation.md) and the
[Rust migration guide](../subagent-cancellation-observation.md).

Approval of a `BeforeNode` checkpoint skips only that exact interrupt once.
Any configured interrupt on a subsequent node is still evaluated. Finish nodes
use the same before/after interrupt semantics as every other executable node;
resuming an after-finish checkpoint completes the preserved path without
executing the finish node twice.

---

## Best Practices

1. **Keep nodes focused**: Each node should do one thing well
2. **Use SharedState keys consistently**: Document input/output keys
3. **Handle errors in function nodes**: Return `Err()` to stop workflow
4. **Use streaming for long-running workflows**: Provide real-time feedback
5. **Design for resumability**: Use checkpoints for critical workflows

---

## When to Use

| Scenario | Suitability | Reason |
|----------|-------------|--------|
| Multi-agent pipelines | ★★★★★ | Natural fit |
| Conditional flows | ★★★★★ | Conditional edges |
| Parallel processing | ★★★★★ | Fan-out/fan-in |
| Simple single-agent | ★☆☆☆☆ | Overkill |

See: `echo-agent-learning/tests/example_contracts/demo39_workflow.rs`, `echo-agent-learning/tests/example_contracts/demo34_workflow_stream.rs`, `echo-agent-learning/tests/example_contracts/demo37_declarative_workflow.rs`
