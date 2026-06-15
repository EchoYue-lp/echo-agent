# Self-Improvement Pipeline — Analyze, Critique, Evolve

## What It Is

The self-improvement pipeline analyzes completed agent runs to detect failure patterns, generate improvement suggestions, and iteratively improve agent performance. It combines eval, trace analysis, prompt regeneration, and background learning into a unified system.

```
Run Agent → Collect Trace → Analyze Failures → Generate Suggestions
                                                      ↓
                                              Human Reviews
                                                      ↓
                                              Apply Changes → Re-evaluate → Loop
```

---

## Problem It Solves

Agent performance degrades over time as tasks get harder and edge cases accumulate. Without systematic improvement:

- **Failure patterns repeat**: The agent writes files without reading them, retries failing tools excessively, or misses obvious tools
- **No feedback loop**: Past failures don't inform future behavior
- **No memory of lessons**: Each session starts from scratch
- **Skill rot**: Skills become outdated without detection

---

## Safety Model

All suggestions require human review. The pipeline does NOT automatically:
- Modify core runtime code
- Relax security policies
- Change permission rules
- Publish or deploy anything

---

## Pipeline Architecture

```
┌─────────────────────────────────────────────────────────┐
│                 Self-Improvement Pipeline                 │
│                                                         │
│  ┌─────────────┐  ┌─────────────┐  ┌────────────────┐  │
│  │  EvalRunner  │  │  Analyzer   │  │PromptGenerator │  │
│  │  (evaluate)  │  │ (detect)    │  │  (improve)     │  │
│  └──────┬──────┘  └──────┬──────┘  └───────┬────────┘  │
│         │                │                  │            │
│  ┌──────▼────────────────▼──────────────────▼────────┐  │
│  │              ImprovementLoop                        │  │
│  │  eval → critique → suggest → re-eval → track best  │  │
│  └───────────────────────────────────────────────────┘  │
│                                                         │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │
│  │BackgroundRev │  │   Curator    │  │TrajectorySaver│  │
│  │(learn from   │  │(skill life-  │  │(fine-tune     │  │
│  │ conversations)│  │ cycle)       │  │ data)         │  │
│  └──────────────┘  └──────────────┘  └──────────────┘  │
└─────────────────────────────────────────────────────────┘
```

---

## Analyzer — Failure Detection

The `Analyzer` examines completed run traces to detect common failure patterns:

```rust
use echo_agent::improve::Analyzer;

let critique = Analyzer::analyze(&run);
println!("{}", critique.format_report());
```

Output:
```
Run: run_abc123
Success: false (score: 0.65)

Issues Found:
  - Write without read: write was called 2 time(s) without prior read_file
  - Excessive retries: shell was retried 4 time(s)

Suggestions:
  [Prompt] tools: Add instruction: 'Always read a file with read_file before editing it.'
  [Policy] force_read_before_edit: true — Reason: Agent wrote files without reading first 2 time(s)
```

### Detected Issue Types

| Issue | Detection Logic | Suggestion |
|-------|----------------|------------|
| `WriteWithoutRead` | Write tool called before any read on same file | Add read-before-write prompt instruction |
| `ExcessiveRetries` | Same tool error > 2 times | Add "try different approach" instruction |
| `ToolErrorPattern` | Repeated failures of same tool | Generate eval case for that tool |
| `ContextOverflow` | Compression was triggered | Suggest context-aware prompting |
| `MissingTool` | Expected tool not used | Suggest adding tool instruction |
| `ExcessiveToolCalls` | > 20 tool calls in one run | Add efficiency instruction |

---

## ImprovementLoop — Iterative Improvement

The loop runs eval, critiques failures, generates suggestions, and tracks improvement:

```rust
use echo_agent::improve::ImprovementLoop;

let loop_runner = ImprovementLoop {
    max_iterations: 5,
    improvement_threshold: 0.95,  // Stop when test score >= 95%
    holdout_ratio: 0.4,           // 40% for testing, 60% for training
};

let result = loop_runner.run(&cases, agent_factory, &run_store).await;

println!("Best score: {:.2} at iteration {}", result.best_score, result.best_iteration);
for iter in &result.iterations {
    println!("  Iter {}: train={:.2}, test={:.2}, {} suggestions",
        iter.iteration, iter.train_score, iter.eval_report.avg_score,
        iter.suggestions.len());
}
```

### How It Works

1. **Stratified Split**: Cases are split by criteria type (TestPass, OutputContains, etc.) to prevent overfitting
2. **Train Eval**: Run agent on training set
3. **Critique**: Load traces for failed runs, analyze with `Analyzer`
4. **Generate Suggestions**: Collect and deduplicate suggestions from all critiques
5. **Test Eval**: Run agent on holdout set (blinded — test scores not visible to generator)
6. **Track Best**: Record best test score across iterations
7. **Early Stop**: If `improvement_threshold` reached, stop early

---

## SelfEvolution — One Switch

The `SelfEvolution` engine is the simplest entry point — one `.enable()` turns everything on:

```rust
use echo_agent::improve::SelfEvolution;

let result = SelfEvolution::new()
    .with_eval_cases(cases)
    .with_run_store(run_store)
    .max_iterations(5)
    .with_report_dir("./eval_reports")
    .enable()
    .run(|| create_agent())
    .await;

if let Some(result) = result {
    println!("Best: {:.2} (iteration {})", result.best_score, result.best_iteration);
    // HTML reports generated in ./eval_reports/
    // - iter_0.html, iter_1.html, ..., final.html
}
```

---

## PromptGenerator — LLM-Powered Prompt Improvement

Uses an LLM to generate improved system prompts based on failure analysis:

```rust
use echo_agent::improve::PromptGenerator;

let generator = PromptGenerator::new();

let improved_prompt = generator.generate_improved_prompt(
    &agent,
    current_system_prompt,
    &critiques,
    "code editing",
).await;

println!("Improved prompt:\n{}", improved_prompt);
```

The generator:
- Takes current prompt + failure critiques + task domain
- Asks LLM to produce improved prompt that addresses failures
- Includes previous suggestions to avoid repeating changes
- Wraps output in `<new_prompt>` tags for reliable extraction
- Limits output to `max_chars` (default 4096)

---

## BackgroundReviewer — Learn from Conversations

After every conversation turn, spawns a background task to extract memory and skill updates:

```rust
use echo_agent::evolution::{BackgroundReviewer, BackgroundReviewConfig};

let reviewer = BackgroundReviewer::new(
    BackgroundReviewConfig {
        enabled: true,
        max_iterations: 8,
        review_memory: true,   // Extract user preferences
        review_skills: true,   // Extract reusable patterns
    },
    llm_client,
    Some(memory_store),
    Some(run_store),
);

// Review a completed run
let outcome = reviewer.review(&run).await?;
println!("Actions: {:?}", outcome.actions);
// e.g., ["Memory reviewed", "Skill update recommended"]
```

### What It Looks For

**Memory signals**:
- User persona, desires, preferences
- Personal details worth remembering
- Expectations about agent behavior

**Skill signals**:
- User corrections ("stop doing X", "too verbose")
- Non-trivial techniques or workarounds
- Outdated or missing skills

**What it ignores**:
- Environment failures (missing binaries)
- Transient errors
- One-off task narratives

---

## Curator — Skill Lifecycle Management

Automatically manages skill lifecycle: Active → Stale → Archived

```rust
use echo_agent::improve::{Curator, CuratorConfig};

let curator = Curator::new(
    CuratorConfig {
        stale_days: 30,    // 30 days inactive → Stale
        archive_days: 90,  // 90 days inactive → Archived
        enabled: true,
    },
    "~/.echo-agent/curator_state.json",
);

// Register a skill when used
curator.touch_skill("my-code-review", true)?;

// Pin important skills (exempt from auto-transitions)
curator.pin_skill("critical-skill")?;

// Apply automatic transitions
let transitions = curator.apply_transitions()?;
for (name, from, to) in &transitions {
    println!("{name}: {from:?} → {to:?}");
}

// Check status
let status = curator.status();
println!("Active: {}, Stale: {}, Archived: {}, Pinned: {}",
    status.active, status.stale, status.archived, status.pinned);
```

### Lifecycle Rules

- Only operates on **agent-created** skills (never bundled/external)
- **Pinned** skills are exempt from auto-transitions
- Never auto-deletes, only archives
- State persisted to JSON file

---

## TrajectorySaver — Fine-Tuning Data

Converts completed runs into ShareGPT-format JSONL for model fine-tuning:

```rust
use echo_agent::improve::TrajectorySaver;

let saver = TrajectorySaver::default_dir()?;

// Save a completed run
saver.save(&run, "qwen3-max").await?;

// List saved trajectories
let entries = saver.list(Some("2026-05-29")).await?;
for entry in &entries {
    println!("{}: {} turns, {} tokens, {} tool calls",
        entry.id, entry.conversations.len(),
        entry.token_usage, entry.tool_call_count);
}

// Get aggregate stats
let stats = saver.stats().await?;
println!("Total: {}, Completed: {}, Avg duration: {}ms",
    stats.total, stats.completed, stats.avg_duration_ms);
```

### Output Format

Each trajectory is stored as a JSON line:
```json
{
  "id": "run_abc123",
  "session_id": "sess_1",
  "conversations": [
    {"from": "human", "value": "Read the file foo.txt"},
    {"from": "gpt", "value": "🔧 Tool Call: read_file\n{\"path\": \"foo.txt\"}"},
    {"from": "tool", "value": "✅ read_file: Hello, world!"},
    {"from": "gpt", "value": "The file foo.txt contains: Hello, world!"}
  ],
  "model": "qwen3-max",
  "completed": true,
  "timestamp": "2026-05-29T10:00:00Z",
  "token_usage": 150,
  "tool_call_count": 1,
  "duration_ms": 500
}
```

---

## CritiqueStore — Pattern Aggregation

Stores critiques and tracks issue patterns over time:

```rust
use echo_agent::improve::CritiqueStore;

let store = CritiqueStore::new();

// Store critiques
store.store(critique);

// Get top-N most frequent patterns
let patterns = store.top_patterns(5);
for (pattern, count) in &patterns {
    println!("{pattern}: {count} occurrences");
}

// Retrieve by run
let critiques = store.get_by_run("run_abc123");
```

---

## Complete Example

```rust
use echo_agent::prelude::*;
use echo_agent::eval::*;
use echo_agent::improve::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Define eval cases
    let cases = vec![
        EvalCase {
            id: "hello".into(),
            name: "Hello world".into(),
            description: "Agent says hello".into(),
            task: "Say hello world".into(),
            project_fixture: None,
            success_criteria: SuccessCriteria::OutputContains {
                substring: "hello".into(),
            },
            constraints: Default::default(),
        },
    ];

    // 2. Enable self-evolution
    let result = SelfEvolution::new()
        .with_eval_cases(cases)
        .max_iterations(3)
        .with_report_dir("./reports")
        .enable()
        .run(|| {
            let config = AgentConfig::new("qwen3-max", "assistant", "You are helpful");
            Box::new(ReactAgent::new(config))
        })
        .await;

    if let Some(result) = result {
        println!("Best score: {:.2}", result.best_score);
    }
    Ok(())
}
```

---

## When to Use

| Component | When | Frequency |
|-----------|------|-----------|
| `Analyzer` | After every failed run | Per-run |
| `ImprovementLoop` | When tuning prompts | Per-session |
| `SelfEvolution` | Full eval + improvement cycle | Per-release |
| `BackgroundReviewer` | After every conversation | Per-turn |
| `Curator` | Periodic skill maintenance | Daily/weekly |
| `TrajectorySaver` | Collect fine-tuning data | Continuous |

---

## Integration with Agent

The self-improvement pipeline is **not built into the agent loop**. It runs externally as a separate analysis and improvement pass. This design keeps the agent lightweight — most users don't need self-improvement in production.

### Feature Gate

Enable with the `improve` feature flag:

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["improve"] }
```

> Note: `improve` depends on `eval` for the evaluation framework. When using `SelfEvolution` or `ImprovementLoop`, also enable `eval`.

### Usage Pattern

```
┌─────────────────────────────────────────────────┐
│  Production (no improve)                         │
│  agent.execute("do the task").await              │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│  Self-Improvement (separate pass)                │
│  SelfEvolution::new()                            │
│      .with_eval_cases(cases)                     │
│      .enable()                                   │
│      .run(agent_factory)                         │
│      .await                                      │
└─────────────────────────────────────────────────┘
```

Self-improvement is a **batch analysis tool**, not a runtime hook. You run it:
- When tuning prompts (ImprovementLoop)
- Before releases (SelfEvolution full cycle)
- Periodically (Curator skill maintenance)
- Continuously (TrajectorySaver data collection)

### How It Differs from Self-Reflection

| Aspect | Self-Reflection | Self-Improvement |
|--------|----------------|------------------|
| When | During agent execution | After agent execution |
| Scope | Single task | Cross-task patterns |
| Feedback | Verbal (LLM critique) | Structural (code analysis) |
| Feature flag | `self-reflection` | `improve` |
| Integration | Built into agent loop | External batch process |

See also: [24 - Eval System](./24-eval-system.md) for the evaluation framework that feeds this pipeline.
