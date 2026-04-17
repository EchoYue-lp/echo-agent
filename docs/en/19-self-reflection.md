# Self-Reflection Agent — Quality Assurance Loop

## What It Is

Self-Reflection Agent introduces an "evaluate → reflect → refine" loop on top of the ReAct paradigm, enabling the Agent to learn from its mistakes through verbal feedback.

```
Actor (generate) → Evaluator (critique) → Reflector (reflect) → Refine → Loop
                           ↓                              ↓
                     Episodic Memory (cross-task experience storage)
```

---

## Problem It Solves

A single-pass LLM response may have:
- **Quality inconsistency**: Different runs produce varying quality
- **No self-correction**: Cannot detect and fix its own errors
- **No learning**: Repeats the same mistakes across tasks

Self-Reflection solves this by adding a quality gate with iterative improvement.

---

## Three-Component Model

```rust
use echo_agent::agent::self_reflection::{SelfReflectionAgent, LlmCritic};

// 1. Generator (Actor): Produces initial output
let generator = ReactAgentBuilder::simple("qwen3-max", "Technical writer")?;

// 2. Critic (Evaluator): Scores and provides feedback
let critic = LlmCritic::new("qwen3-max")
    .with_pass_threshold(8.0);  // 0-10 scale, pass if >= 8.0

// 3. Reflection Agent: Orchestrates the loop
let mut agent = SelfReflectionAgent::new("reflection_agent", generator, critic)
    .max_reflections(3);        // Max 3 refinement iterations
```

---

## Execution Flow

```
execute(task)
    │
    ├─ 1. Build memory context from past experiences
    │
    ├─ 2. Generate initial response
    │      generator.execute(enhanced_task)
    │
    └─ Loop (max_reflections times):
          │
          ├─ Critique: critic.critique(task, answer)
          │     ├─ score: 0.0 - 10.0
          │     ├─ passed: score >= threshold
          │     └─ feedback: improvement suggestions
          │
          ├─ If passed → Store success experience → Return answer
          │
          └─ If failed:
                ├─ Reflection: Analyze what went wrong
                ├─ Refinement: Generate improved answer
                ├─ Extract experience lesson
                └─ Continue loop
```

---

## Critic Types

### StaticCritic

Simple rule-based evaluation:

```rust
use echo_agent::agent::self_reflection::StaticCritic;

let critic = StaticCritic::always_pass();  // Always passes (for testing)
let critic = StaticCritic::always_fail();  // Always fails (for testing)
let critic = StaticCritic::with_threshold(7.0, |output| {
    // Custom scoring function
    if output.len() > 100 { 7.5 } else { 5.0 }
});
```

### LlmCritic

LLM-based semantic evaluation:

```rust
use echo_agent::agent::self_reflection::LlmCritic;

let critic = LlmCritic::new("qwen3-max")
    .with_pass_threshold(8.0)
    .with_prompt(|task, output| format!(
        "Evaluate this answer (0-10 scale):\nTask: {}\nAnswer: {}\n\nProvide score and improvement suggestions.",
        task, output
    ));
```

### CompositeCritic

Combine multiple critics:

```rust
use echo_agent::agent::self_reflection::{CompositeCritic, CompositeStrategy};

let critic = CompositeCritic::new()
    .add_critic(Box::new(length_critic))
    .add_critic(Box::new(llm_critic))
    .with_strategy(CompositeStrategy::AllMustPass);  // or AnyCanPass
```

---

## Episodic Memory

The agent stores lessons learned across tasks:

```rust
pub struct ReflectionExperience {
    pub lesson: String,        // What was learned
    pub error_pattern: String, // Identified error pattern
    pub use_count: usize,      // Reference count (for eviction)
}
```

When a new task arrives, relevant past experiences are injected into the context:

```
Original task: "Explain Rust ownership"

Enhanced task: "Explain Rust ownership

Reference these past lessons:
1. Always mention borrow checker when discussing ownership
2. Include code examples for move semantics
3. Avoid confusing 'lifetime' with 'scope'"
```

---

## Usage Example

```rust
use echo_agent::prelude::*;
use echo_agent::agent::self_reflection::{SelfReflectionAgent, LlmCritic};

#[tokio::main]
async fn main() -> Result<()> {
    let generator = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("writer")
        .system_prompt("You are a technical documentation writer.")
        .build()?;

    let critic = LlmCritic::new("qwen3-max")
        .with_pass_threshold(8.0);

    let mut agent = SelfReflectionAgent::new("reflection_agent", generator, critic)
        .max_reflections(3)
        .pass_threshold(7.0);

    let result = agent
        .execute("Explain Rust ownership clearly and accurately.")
        .await?;

    println!("Final result:\n{}", result);
    Ok(())
}
```

---

## Streaming Events

```rust
let mut stream = agent.execute_stream(task).await?;

while let Some(event) = stream.next().await {
    match event? {
        AgentEvent::ReflectionStart { iteration } => {
            println!("Starting reflection round {}", iteration + 1);
        }
        AgentEvent::CritiqueGenerated { score, passed, feedback } => {
            println!("Score: {:.1}/10 ({})", score, if passed { "PASS" } else { "FAIL" });
        }
        AgentEvent::Refining { iteration } => {
            println!("Refining answer...");
        }
        AgentEvent::ReflectionEnd { iteration, score, passed } => {
            println!("Round {} complete: {:.1}", iteration + 1, score);
        }
        AgentEvent::FinalAnswer(answer) => {
            println!("Final: {}", answer);
            break;
        }
        _ => {}
    }
}
```

---

## ReflectiveExecutor

Use Self-Reflection as the Executor in Plan-and-Execute:

```rust
use echo_agent::agent::plan_execute::{PlanExecuteAgent, LlmPlanner};
use echo_agent::agent::self_reflection::{SelfReflectionAgent, LlmCritic, ReflectiveExecutor};

let generator = ReactAgentBuilder::simple("qwen3-max", "Step executor")?;
let critic = LlmCritic::new("qwen3-max");
let reflective_agent = SelfReflectionAgent::new("reflective", generator, critic);

let executor = ReflectiveExecutor::new(reflective_agent);
let planner = LlmPlanner::new("qwen3-max");

let mut agent = PlanExecuteAgent::new("plan_agent", planner, executor);
```

Each plan step now goes through the quality loop.

---

## Best Practices

1. **Set appropriate threshold**: 7.0-8.0 is usually good; too high causes infinite loops
2. **Limit reflections**: 2-3 iterations is usually enough
3. **Use domain-specific critics**: Custom scoring functions for specific tasks
4. **Monitor episodic memory**: Clear if it grows too large or contains stale lessons

---

## When to Use

| Scenario | Suitability | Reason |
|----------|-------------|--------|
| High-quality writing | ★★★★★ | Multiple polishing rounds |
| Code generation | ★★★★☆ | Can detect logic errors |
| Fact-checking | ★★★★☆ | Critic can verify facts |
| Simple Q&A | ★☆☆☆☆ | Over-engineered, high cost |

See: `examples/demo20_audit.rs`