# Core AI Agent Patterns

This document describes the core AI Agent patterns implemented in echo-agent, drawing from cutting-edge research in academia and industry.

---

## Table of Contents

1. [ReAct Pattern](#react-pattern)
2. [Plan-and-Execute Pattern](#plan-and-execute-pattern)
3. [Self-Reflection Pattern](#self-reflection-pattern)
4. [LangGraph Workflow Pattern](#langgraph-workflow-pattern)
5. [Pattern Comparison and Selection](#pattern-comparison-and-selection)

---

## ReAct Pattern

### Origin

ReAct (Reasoning + Acting) was proposed by Yao et al. in 2022 [1], interleaving **Reasoning** and **Acting** to form a Thought → Action → Observation loop.

### Core Concept

```
┌─────────────────────────────────────────────────────────────┐
│                     ReAct Loop                               │
│                                                              │
│   User Query                                                │
│       │                                                      │
│       ▼                                                      │
│   ┌─────────────┐     ┌─────────────┐     ┌──────────────┐  │
│   │   Thought   │────▶│   Action    │────▶│  Observation │  │
│   │ (Reasoning) │     │ (Tool Call) │     │   (Result)   │  │
│   └─────────────┘     └─────────────┘     └──────────────┘  │
│          ▲                                        │          │
│          └────────────────────────────────────────┘          │
│                                                              │
│   Termination: Generate Final Answer or reach max iterations │
└─────────────────────────────────────────────────────────────┘
```

### echo-agent Implementation

```rust
// src/agents/react/run.rs
pub async fn run_react_loop(&mut self, task: &str) -> Result<String> {
    loop {
        // 1. Think: Call LLM to generate next step
        let response = self.think().await?;
        
        match response.step_type {
            StepType::FinalAnswer(answer) => {
                // Termination condition reached
                return Ok(answer);
            }
            StepType::ToolCalls(calls) => {
                // 2. Act: Execute tools
                for call in calls {
                    let result = self.tool_manager.execute(&call).await?;
                    // 3. Observe: Add result to context
                    self.context.push_tool_result(call.id, result);
                }
            }
        }
        
        // Check max iterations
        if iterations >= self.config.max_iterations {
            return Err(ReactError::MaxIterationsExceeded);
        }
    }
}
```

### Chain-of-Thought (CoT) Enhancement

echo-agent enables CoT by default, requiring the LLM to describe its reasoning in natural language before making tool calls:

```rust
const COT_INSTRUCTION: &str = "Before calling a tool, briefly describe your analysis and execution plan in text.";
```

### Suitable Scenarios

| Scenario | Suitability | Reason |
|----------|-------------|--------|
| Multi-tool orchestration | ★★★★★ | Dynamic tool selection with flexible composition |
| Open-ended Q&A | ★★★★★ | Traceable chain of thought, debuggable |
| Precise computation | ★★★☆☆ | Reasoning errors may lead to incorrect results |
| Long-running tasks | ★★☆☆☆ | High iteration cost, prone to token overflow |

### References

[1] Yao, S., et al. "ReAct: Synergizing Reasoning and Acting in Language Models." ICLR 2023.

---

## Plan-and-Execute Pattern

### Origin

Plan-and-Execute was proposed by Wei et al. [2], decomposing complex tasks into explicit **Planning** and **Execution** phases.

### Core Concept

```
┌─────────────────────────────────────────────────────────────────────┐
│                  Plan-and-Execute Pipeline                           │
│                                                                      │
│   Phase 1: Planning                                                 │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  Task → [Planner] → Plan { step_1, step_2, ..., step_n }   │   │
│   └─────────────────────────────────────────────────────────────┘   │
│                              │                                       │
│                              ▼                                       │
│   Phase 2: Execution (DAG)                                          │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │                                                              │   │
│   │     step_1 ──┬──▶ step_2 ──┬──▶ step_4 ──▶ Summary        │   │
│   │              │              │                               │   │
│   │              └──▶ step_3 ──┘                               │   │
│   │                                                              │   │
│   │  [TaskManager] Schedules in topological order,              │   │
│   │  independent steps execute in parallel                       │   │
│   └─────────────────────────────────────────────────────────────┘   │
│                                                                      │
│   Phase 3: Replanning (on failure)                                  │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  step_2 fails → Identify affected downstream steps          │   │
│   │               → Replan subgraph                             │   │
│   └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

### echo-agent Implementation

```rust
// src/agents/plan_execute/mod.rs

// Planner trait: Converts a task into a plan
pub trait Planner: Send + Sync {
    fn plan<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<Plan>>;
}

// Executor trait: Executes a single step
pub trait Executor: Send + Sync {
    fn execute_step<'a>(
        &'a mut self,
        step_description: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<String>>;
}

// Plan-and-Execute Agent
pub struct PlanExecuteAgent<P: Planner, E: Executor> {
    planner: P,
    executor: E,
    max_replans: usize,
    execution_mode: ExecutionMode, // Sequential | Parallel
}
```

### Plan → Task DAG Conversion

```rust
impl Plan {
    pub fn to_task_dag(&self) -> Vec<Task> {
        self.steps.iter().enumerate().map(|(i, step)| {
            Task {
                id: format!("plan_step_{}", i),
                description: step.description.clone(),
                dependencies: step.dependencies.iter()
                    .map(|d| format!("plan_step_{}", d))
                    .collect(),
                ..Default::default()
            }
        }).collect()
    }
}
```

### Incremental Replanning

When a step fails, only the affected downstream subgraph is replanned, not the entire plan:

```rust
// Use downstream_steps_recursive to identify the affected scope
let affected: Vec<usize> = plan.downstream_steps_recursive(failed_step_idx);

// Remove only affected tasks, preserving successful ones
for id in affected {
    task_manager.delete_task(&format!("plan_step_{}", id));
}

// Regenerate sub-plan
let new_plan = planner.plan(&replan_prompt).await?;
```

### Suitable Scenarios

| Scenario | Suitability | Reason |
|----------|-------------|--------|
| Structured tasks | ★★★★★ | Explicit steps, predictable |
| Multi-step pipelines | ★★★★★ | DAG scheduling, parallelizable |
| Failure recovery needed | ★★★★☆ | Supports incremental replanning |
| Open-ended exploration | ★★☆☆☆ | Difficult to plan in advance |

### References

[2] Wei, J., et al. "Chain-of-Thought Prompting Elicits Reasoning in Large Language Models." NeurIPS 2022.

---

## Self-Reflection Pattern

### Origin

Self-Reflection is inspired by Reflexion [3] and CRITIC [4], improving output quality through a "Generate → Evaluate → Refine" closed loop.

### Core Concept

```
┌─────────────────────────────────────────────────────────────────────┐
│                   Self-Reflection Loop                               │
│                                                                      │
│   ┌──────────────┐                                                  │
│   │    Actor     │ ─── Generate initial output ───────────────┐    │
│   │ (Generator)  │                                            │    │
│   └──────────────┘                                            │    │
│                                                               ▼    │
│   ┌──────────────┐     ┌─────────────────┐                  │    │
│   │   Evaluator  │ ──▶ │ score < threshold?│                  │    │
│   │  (Critic)    │     └────────┬────────┘                  │    │
│   └──────────────┘              │                           │    │
│                    ┌────────────┴────────────┐              │    │
│                    │                         │              │    │
│                   Yes                       No             │    │
│                    │                         │              │    │
│                    ▼                         ▼              │    │
│   ┌──────────────┐              ┌──────────────────┐       │    │
│   │   Reflector  │              │  Return final    │       │    │
│   │              │              │     result       │       │    │
│   └──────────────┘              └──────────────────┘       │    │
│          │                                                 │    │
│          │ Reflection text + correction suggestions         │    │
│          ▼                                                 │    │
│   ┌──────────────┐                                         │    │
│   │   Refiner    │ ◀───────────────────────────────────────┘    │
│   │              │                                                  │
│   └──────────────┘                                                  │
│          │                                                          │
│          └──────────▶ Re-enter evaluation ◀────────────────────────┘
│                                                                      │
│   Episodic Memory: Cross-task experience storage                    │
│   to avoid repeating mistakes                                       │
└─────────────────────────────────────────────────────────────────────┘
```

### echo-agent Implementation

```rust
// src/agents/self_reflection/mod.rs

pub struct SelfReflectionAgent<C: Critic> {
    generator: Box<dyn Agent>,        // Actor
    critic: C,                         // Evaluator
    refinement_prompt_builder: Box<dyn RefinementPromptBuilder>, // Reflector
    reflection_prompt_builder: Box<dyn ReflectionPromptBuilder>, // Refiner
    episodic_memory: Vec<ReflectionExperience>, // Cross-task experience
}

// Critic trait: Evaluates output quality
pub trait Critic: Send + Sync {
    fn critique<'a>(
        &'a self,
        task: &'a str,
        output: &'a str,
        context: &'a str,
    ) -> BoxFuture<'a, Result<Critique>>;
}

pub struct Critique {
    pub score: f64,           // 0.0 - 10.0
    pub passed: bool,         // score >= threshold
    pub feedback: String,     // Improvement suggestions
    pub suggestions: Vec<String>,
}
```

### Episodic Memory

```rust
pub struct ReflectionExperience {
    pub lesson: String,       // Lesson learned from the mistake
    pub error_pattern: String,// Identified error pattern
    pub use_count: usize,     // Reference count (used for eviction)
}

// Inject historical experience for new tasks
fn build_memory_context(&self) -> String {
    self.episodic_memory.iter()
        .map(|exp| format!("- {}", exp.lesson))
        .collect::<Vec<_>>()
        .join("\n")
}
```

### LLM-based Critic

```rust
// src/agents/self_reflection/llm_critic.rs
pub struct LlmCritic {
    model: String,
    pass_threshold: f64,
}

impl Critic for LlmCritic {
    async fn critique(&self, task: &str, output: &str, context: &str) -> Result<Critique> {
        let prompt = format!(
            "Evaluate the quality of the following answer (0-10):\nTask: {}\nAnswer: {}\n\nPlease provide a score and improvement suggestions.",
            task, output
        );
        // LLM call and parse structured output
    }
}
```

### Suitable Scenarios

| Scenario | Suitability | Reason |
|----------|-------------|--------|
| High-quality writing | ★★★★★ | Multiple refinement rounds, quality controlled |
| Code generation | ★★★★☆ | Can detect logic errors |
| Fact checking | ★★★★☆ | Evaluator can verify facts |
| Simple Q&A | ★☆☆☆☆ | Over-engineered, high cost |

### References

[3] Shinn, N., et al. "Reflexion: Language Agents with Verbal Reinforcement Learning." NeurIPS 2023.

[4] Gou, Z., et al. "CRITIC: Large Language Models Can Self-Correct with Tool-Interactive Critiquing." ICLR 2024.

---

## LangGraph Workflow Pattern

### Origin

LangGraph [5] was proposed by the LangChain team as a directed graph workflow framework, modeling Agent orchestration as a **state machine + graph structure**.

### Core Concepts

```
┌─────────────────────────────────────────────────────────────────────┐
│                    LangGraph-style Workflow                          │
│                                                                      │
│   SharedState: KV store shared between nodes                         │
│                + structured message history                          │
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
│               │Node E  │ │Node F  │  (parallel fan-out)           │
│               └────────┘ └────────┘                                │
│                    │         │                                     │
│                    └────┬────┘                                     │
│                         ▼                                           │
│                    ┌────────┐                                       │
│                    │  END   │  (fan-in)                            │
│                    └────────┘                                       │
└─────────────────────────────────────────────────────────────────────┘
```

### echo-agent Implementation

```rust
// src/workflow/mod.rs

// Shared state
pub struct SharedState {
    data: HashMap<String, Value>,
    messages: Vec<Message>,
}

// Graph definition
pub struct Graph {
    name: String,
    nodes: HashMap<String, Node>,
    edges: Vec<Edge>,
    entry: String,
    finish: Vec<String>,
}

// Node types
pub enum Node {
    Agent(SharedAgent),
    Function(BoxedFutureFn),
}

// Edge types
pub enum Edge {
    Simple { from: String, to: String },
    Conditional { from: String, condition: ConditionFn, branches: HashMap<String, String> },
}
```

### GraphBuilder DSL

```rust
let graph = GraphBuilder::new("pipeline")
    // Agent node
    .add_agent_node("researcher", researcher)
        .input_key("task")
        .output_key("research")
    // Function node
    .add_function_node("transform", |state| Box::pin(async move {
        let data: String = state.get("research").unwrap_or_default();
        state.set("report", format!("Report: {}", data));
        Ok(())
    }))
    // Conditional edge
    .add_conditional_edge("researcher", |state| {
        let complexity: f64 = state.get("complexity").unwrap_or(0.5);
        if complexity > 0.7 { "detailed_analysis" } else { "summary" }
    })
    // Parallel fan-out
    .add_edge("transform", "review")
    .add_edge("transform", "format")
    // fan-in
    .add_edge("review", "final")
    .add_edge("format", "final")
    .set_entry("researcher")
    .set_finish("final")
    .build()?;
```

### Workflow Types

| Type | Description | echo-agent Implementation |
|------|-------------|---------------------------|
| Sequential | Sequential execution, output of one step is input to the next | `SequentialWorkflow` |
| Concurrent | All Agents execute in parallel, results are merged | `ConcurrentWorkflow` |
| DAG | Executes in topological order, independent nodes auto-parallelize | `DagWorkflow` |
| Graph | LangGraph-style, supports conditional edges and cycles | `Graph` |

### Declarative YAML Workflows

```yaml
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
let graph = Graph::from_yaml("workflow.yaml")?;
let result = graph.run(state).await?;
```

### References

[5] LangGraph Documentation. https://langchain-ai.github.io/langgraph/

---

## Pattern Comparison and Selection

### Decision Tree

```
Task type?
│
├─ Open-ended Q&A / tool orchestration ──▶ ReAct
│   └─ Requires flexible tool and path selection
│
├─ Structured multi-step tasks ──▶ Plan-and-Execute
│   └─ Steps can be predefined, failure recovery needed
│
├─ High-quality output requirements ──▶ Self-Reflection
│   └─ Requires multiple refinement rounds and quality assurance
│
└─ Complex Agent orchestration ──▶ Graph Workflow
    └─ Multi-Agent collaboration, conditional branching, parallelism
```

### Complexity Comparison

| Pattern | Token Cost | Latency | Debuggability | Suitable Complexity |
|---------|-----------|---------|---------------|---------------------|
| ReAct | Medium | Medium | ★★★★☆ | Medium |
| Plan-and-Execute | High | High | ★★★★★ | High |
| Self-Reflection | Very High | Very High | ★★★☆☆ | Medium-High |
| Graph Workflow | Variable | Variable | ★★★★★ | Very High |

### Combined Usage

Patterns can be combined:

```rust
// Self-Reflection as the Executor for Plan-and-Execute
let reflective_executor = ReflectiveExecutor::new(
    SelfReflectionAgent::new("step_agent", generator, critic)
);

let agent = PlanExecuteAgent::new("planner", planner, reflective_executor);
```

---

## Summary

echo-agent implements four core patterns of modern AI Agents:

1. **ReAct** - Dynamic reasoning and action, suitable for tool orchestration
2. **Plan-and-Execute** - Explicit planning and execution, suitable for structured tasks
3. **Self-Reflection** - Quality closed loop, suitable for high-demand outputs
4. **Graph Workflow** - Complex orchestration, suitable for multi-Agent collaboration

Choosing the right pattern is key to building efficient Agents.