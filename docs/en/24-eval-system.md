# Eval System — Measurable, Repeatable Agent Testing

## What It Is

The eval system provides a structured framework for defining test cases, running them against agents, and scoring results. It builds on the trace system to link eval results to execution traces for debugging and regression detection.

```
Define Cases → Run Agent → Score Results → Generate Report
                    ↓
              Trace Linkage (run_id, tool calls, tokens, file changes)
                    ↓
              HTML Report / A/B Comparison / Regression Suite
```

---

## Problem It Solves

Without a structured eval system, agent quality is assessed by "vibes" — manual spot-checks that don't scale and can't detect regressions. The eval system solves:

- **No measurable quality**: Success criteria are subjective and inconsistent
- **No regression detection**: A prompt change that breaks 3 out of 10 cases goes unnoticed
- **No A/B comparison**: Cannot objectively compare two prompt strategies
- **No constraint enforcement**: Agent writes to forbidden paths or makes excessive tool calls without detection

---

## Core Concepts

### EvalCase

A single test case — what to test and how to judge success:

```rust
use echo_agent::eval::{EvalCase, SuccessCriteria, EvalConstraints};

let case = EvalCase {
    id: "read_before_edit_001".into(),
    name: "Read before edit".into(),
    description: "Agent should read a file before editing it".into(),
    task: "Fix the typo in src/main.rs".into(),
    project_fixture: Some(PathBuf::from("fixtures/main_with_typo")),
    success_criteria: SuccessCriteria::AllOf(vec![
        SuccessCriteria::ToolUsed { tool_name: "read_file".into() },
        SuccessCriteria::TestPass { command: "cargo test".into() },
    ]),
    constraints: EvalConstraints {
        required_read_before_edit: true,
        max_files_changed: Some(1),
        forbidden_paths: vec!["Cargo.toml".into()],
        ..Default::default()
    },
};
```

### SuccessCriteria

Seven criteria types for flexible evaluation:

| Criteria | Description | Use Case |
|----------|-------------|----------|
| `TestPass` | Shell command exits 0 | `cargo test`, `pytest`, `npm test` |
| `OutputContains` | Final output contains substring | Simple keyword checks |
| `ToolUsed` | Agent called a specific tool | Verify tool selection |
| `ToolNotUsed` | Agent did NOT call a tool | Verify tool avoidance |
| `AllOf` | All sub-criteria pass | Compound checks |
| `AnyOf` | At least one sub-criteria passes | Flexible alternatives |
| `LlmGraded` | LLM-as-Judge assertions | Semantic quality checks |
| `SweBench` | Clone repo, apply patch, run tests | Benchmark-style eval |

### EvalConstraints

Guardrails the agent must respect:

```rust
EvalConstraints {
    max_files_changed: Some(3),           // Limit blast radius
    max_tool_calls: Some(15),             // Prevent runaway loops
    forbidden_paths: vec![".env".into()], // Protect sensitive files
    required_read_before_edit: true,      // Enforce read-before-write
}
```

---

## EvalRunner

The runner executes cases against an agent and collects results:

```rust
use echo_agent::eval::EvalRunner;
use std::path::PathBuf;

let runner = EvalRunner::new(PathBuf::from("/tmp/eval_workspace"))
    .with_run_store(run_store)  // For trace linkage
    .with_grader(grader, grading_agent);  // For LlmGraded criteria

// Run a single case
let result = runner.run(&case, &agent).await;
println!("Score: {:.2}, Success: {}", result.score, result.success);

// Run all cases with fresh agents per case
let report = runner.run_all(&cases, || create_agent()).await;
println!("Passed: {}/{}", report.passed, report.total);
```

The runner automatically:
- Copies `project_fixture` to a temporary workspace before each run
- Links results to execution traces via `run_id`
- Populates metrics from traces (tool calls, tokens, file changes)
- Evaluates constraints against the trace

---

## LlmGrader — LLM-as-Judge

For semantic quality evaluation that can't be expressed as simple substring checks:

```rust
use echo_agent::eval::{LlmGrader, grader::Assertion};

let grader = LlmGrader::new();

let assertions = vec![
    Assertion {
        id: "accuracy".into(),
        check: "The explanation correctly describes Rust ownership".into(),
        expected: "All three ownership rules are mentioned".into(),
    },
    Assertion {
        id: "clarity".into(),
        check: "The explanation is clear and well-structured".into(),
        expected: "Uses code examples and analogies".into(),
    },
];

let report = grader.grade(&grading_agent, task, output, &assertions).await;
println!("Pass rate: {:.0}%", report.pass_rate * 100.0);

for result in &report.results {
    println!("  [{}] {} (confidence: {:.1})",
        if result.passed { "PASS" } else { "FAIL" },
        result.assertion_id,
        result.confidence,
    );
}
```

The grader:
- Sends task + output + assertions to an LLM
- Returns per-assertion pass/fail with confidence and evidence
- Falls back to keyword matching if JSON parsing fails

---

## A/B Comparison

Compare two agent configurations on the same cases:

```rust
use echo_agent::eval::AbComparator;

let comparison = AbComparator::compare(
    &cases,
    || create_agent_with_prompt_v1(),
    || create_agent_with_prompt_v2(),
).await;

println!("{}", AbComparator::format_summary(&comparison));
// Output:
// A/B Comparison Results:
//   Baseline:  0.7200 avg (n=10)
//   Experiment: 0.8500 avg (n=10)
//   Delta: +0.1300 → improved
//   Improved: 6  Regressed: 1  Unchanged: 3
```

---

## Regression Suite

Build eval cases from past successful runs to detect regressions:

```rust
use echo_agent::eval::RegressionSuite;

// Build from historical traces
let suite = RegressionSuite::from_traces(&past_runs);
println!("{} regression cases from traces", suite.len());

// Add manual cases
let suite = suite.with_case(EvalCase { /* ... */ });

// Run regression check
let report = suite.run_all(&runner, &agent).await;
if report.failed > 0 {
    println!("REGRESSION: {} cases that previously passed now fail!", report.failed);
}
```

---

## Trajectory Replay

Offline analysis of past runs without re-running the agent:

```rust
use echo_agent::eval::TrajectoryReplay;

let replay = TrajectoryReplay::new(run);

// Analyze tool usage
let counts = replay.tool_call_counts();
for (tool, count) in &counts {
    println!("  {tool}: {count} calls");
}

// Detect policy violations
let violations = replay.detect_write_without_read();
for v in &violations {
    println!("VIOLATION: {v}");
}

// Generate metrics
let metrics = replay.to_metrics(&constraints);
for m in &metrics {
    println!("  {}: {:.2} — {}", m.name, m.score, m.detail);
}
```

---

## Trigger Accuracy

Measure how well sub-agent routing works:

```rust
use echo_agent::eval::{TriggerAccuracy, trigger::TriggerTestCase};

let cases = vec![
    TriggerTestCase {
        query: "read src/main.rs".into(),
        expected_agent: "code-explorer".into(),
        should_trigger: true,
        runs_per_query: 5,  // Run 5 times for stability
    },
    TriggerTestCase {
        query: "what is 2+2".into(),
        expected_agent: "".into(),
        should_trigger: false,
        runs_per_query: 1,
    },
];

let accuracy = TriggerAccuracy::evaluate(&cases, &actual_triggers);
println!("Precision: {:.2}", accuracy.precision);
println!("Recall: {:.2}", accuracy.recall);
println!("F1: {:.2}", accuracy.f1);
```

---

## HTML Reports

Generate self-contained HTML reports for sharing and review:

```rust
use echo_agent::eval::generate_html;

// Static report
let html = generate_html(&report, "Eval Report v2.1");
std::fs::write("report.html", html)?;

// Interactive review with feedback
use echo_agent::eval::server::generate_review_html;
let html = generate_review_html(&report, "Review Session");
std::fs::write("review.html", html)?;
```

The static report includes:
- Summary cards (passed/total, avg score, min/max, std dev, tool calls, tokens)
- Per-case pass/fail matrix with score, tool calls, file changes, duration
- Filter buttons (All / Passed / Failed)

The interactive review adds:
- Click-to-expand feedback forms
- JSON feedback export

---

## Complete Example

```rust
use echo_agent::prelude::*;
use echo_agent::eval::*;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Define cases
    let cases = vec![
        EvalCase {
            id: "hello_001".into(),
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

    // 2. Create runner
    let runner = EvalRunner::new(PathBuf::from("/tmp/eval"));

    // 3. Run with factory
    let report = runner.run_all(&cases, || {
        let config = AgentConfig::new("qwen3-max", "assistant", "You are helpful");
        Box::new(ReactAgent::new(config))
    }).await;

    // 4. Generate report
    let html = generate_html(&report, "My Eval");
    std::fs::write("eval_report.html", html)?;

    println!("Results: {}/{} passed", report.passed, report.total);
    Ok(())
}
```

---

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                    EvalRunner                         │
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐   │
│  │ EvalCase  │  │ Fixture  │  │ SuccessCriteria  │   │
│  │  (task)   │  │  (copy)  │  │  (judge)         │   │
│  └──────────┘  └──────────┘  └──────────────────┘   │
│                                                      │
│  ┌──────────────────────────────────────────────┐    │
│  │              Agent.execute(task)               │    │
│  └──────────────────┬───────────────────────────┘    │
│                     │                                 │
│  ┌──────────────────▼───────────────────────────┐    │
│  │              Trace (Run)                       │    │
│  │  run_id, events, token_usage, timings          │    │
│  └──────────────────┬───────────────────────────┘    │
│                     │                                 │
│  ┌──────────────────▼───────────────────────────┐    │
│  │              EvalResult                        │    │
│  │  success, score, metrics, violations           │    │
│  └──────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────┘
         │                    │
         ▼                    ▼
┌─────────────────┐  ┌─────────────────┐
│   EvalReport    │  │  HTML Report    │
│  (aggregate)    │  │  (visualize)    │
└─────────────────┘  └─────────────────┘
```

---

## Integration with Agent

The eval system is **not built into the agent loop**. It runs externally as a separate evaluation pass. This design keeps the agent lightweight — most users don't need eval in production.

### Feature Gate

Enable with the `eval` feature flag:

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["eval"] }
```

### Usage Pattern

```
┌─────────────────────────────────────────────────┐
│  Production (no eval)                            │
│  agent.execute("do the task").await              │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│  Evaluation (separate pass)                      │
│  let runner = EvalRunner::new(workspace);        │
│  let report = runner.run_all(&cases, factory);   │
│  generate_html(&report, "Report");               │
└─────────────────────────────────────────────────┘
```

Eval is a **post-hoc analysis tool**, not a runtime hook. You run it:
- After prompt changes (A/B comparison)
- Before releases (regression suite)
- Periodically (CI/CD quality gates)

### AgentRunner Integration

The `AgentRunner` high-level builder supports attaching an `EvalRunner` for future auto-evaluation:

```rust
use echo_agent::runner::AgentRunner;
use echo_agent::eval::EvalRunner;

let runner = AgentRunner::new()
    .model("qwen3-max")
    .with_eval_recorder(EvalRunner::new("/tmp/eval"))
    .build()?;
```

> **Status**: The eval recorder is stored but not yet executed automatically. Use `EvalRunner::run()` manually.

See also: [25 - Self-Improvement](./25-self-improvement.md) for how eval feeds into the improvement loop.
