# Headless Mode

> **Status: Implemented.**
> The `headless` module provides a single-prompt, non-interactive execution mode designed for CI/CD pipelines, scripting, and automation workflows.

---

## What It Is

Headless mode runs the agent with a single prompt, collects the output, and exits — no interactive REPL, no TUI, no user-in-the-loop. It is the right entry point when you want the agent to behave like a traditional CLI tool: read input, produce output, return an exit code.

Typical use cases:
- **CI/CD pipelines** — automated code review, test generation, documentation updates
- **Scripting** — chain the agent with shell pipes, `cron` jobs, or Makefile targets
- **Batch processing** — run the same prompt across many repositories or datasets
- **Non-interactive automation** — any context where no human is present to answer questions

---

## HeadlessConfig

`HeadlessConfig` controls every aspect of a headless run:

```rust
pub struct HeadlessConfig {
    /// The prompt to execute.
    pub prompt: String,

    /// Exit with error if the agent reports failure.
    pub exit_on_error: bool,

    /// Output format: "text" (default) or "json".
    pub output_format: String,

    /// Max iterations before forcing stop (safety limit).
    pub max_iterations: Option<usize>,
}
```

| Field | Type | Default | Purpose |
|---|---|---|---|
| `prompt` | `String` | `""` (empty) | The prompt sent to the agent. An empty prompt returns an error immediately without building the agent. |
| `exit_on_error` | `bool` | `true` | When `true`, the process exits with code 1 on agent failure. |
| `output_format` | `String` | `"text"` | Controls `format_output()`: `"text"` returns raw output; `"json"` wraps the result in a structured JSON envelope. |
| `max_iterations` | `Option<usize>` | `None` | Safety cap on ReAct loop iterations. Prevents runaway execution in unattended environments. |

---

## run_headless() API

```rust
pub async fn run_headless<F>(config: HeadlessConfig, configure: F) -> HeadlessResult
where
    F: FnOnce(ReactAgentBuilder) -> ReactAgentBuilder,
```

The function takes two arguments:

1. **`config`** — the `HeadlessConfig` described above.
2. **`configure`** — a closure that receives a `ReactAgentBuilder` and returns it after customization. This is where you set the model, system prompt, tools, and any other agent-level configuration.

The builder-pattern closure gives full control over agent construction without exposing internal lifecycle management:

```rust
let result = run_headless(config, |builder| {
    builder
        .model("qwen3-max")
        .system_prompt("You are a code review assistant")
        .tool(Box::new(FileReadTool))
        .tool(Box::new(ShellTool))
})
.await;
```

### Execution flow

```
run_headless(config, configure)
    │
    ├─ Empty prompt?  → return error HeadlessResult immediately
    │
    ├─ ReactAgentBuilder::new()
    │       │
    │       └─ configure(builder)   ← caller customizes here
    │               │
    │               └─ max_iterations applied if set
    │                       │
    │                       └─ builder.build()
    │                               │
    │                               ├─ Build error? → return error HeadlessResult
    │                               │
    │                               └─ agent.execute(&prompt)
    │                                       │
    │                                       ├─ Ok → HeadlessResult { success: true, ... }
    │                                       └─ Err → HeadlessResult { success: false, ... }
    │
    └─ return HeadlessResult
```

---

## HeadlessResult

```rust
pub struct HeadlessResult {
    /// The agent's final output text.
    pub output: String,

    /// Whether the execution succeeded.
    pub success: bool,

    /// Model name used.
    pub model: String,

    /// Output format requested.
    pub format: String,
}
```

### Key methods

#### exit_code()

```rust
pub fn exit_code(&self) -> i32
```

Returns `0` on success, `1` on failure — suitable for direct use with `std::process::exit()`.

#### format_output()

```rust
pub fn format_output(&self) -> String
```

Formats the result for stdout according to the requested `output_format`:

- **`"text"`** — returns `output` as-is.
- **`"json"`** — returns a pretty-printed JSON envelope:

```json
{
  "success": true,
  "model": "qwen3-max",
  "output": "..."
}
```

---

## Examples

### Example 1: Basic Headless Execution

Run a single prompt, print the output, and exit with the appropriate code:

```rust
use echo_agent::headless::{HeadlessConfig, run_headless};

#[tokio::main]
async fn main() {
    let config = HeadlessConfig {
        prompt: "List all Rust files in the project".into(),
        exit_on_error: true,
        output_format: "text".into(),
        max_iterations: Some(10),
    };

    let result = run_headless(config, |builder| builder).await;
    println!("{}", result.format_output());
    std::process::exit(result.exit_code());
}
```

### Example 2: Customizing the Agent

Provide a model, system prompt, and tools through the builder closure:

```rust
use echo_agent::headless::{HeadlessConfig, run_headless};

#[tokio::main]
async fn main() {
    let config = HeadlessConfig {
        prompt: "Review src/main.rs for security issues".into(),
        exit_on_error: true,
        output_format: "text".into(),
        max_iterations: Some(20),
    };

    let result = run_headless(config, |builder| {
        builder
            .model("qwen3-max")
            .system_prompt("You are a security-focused code reviewer.")
            .tool(Box::new(FileReadTool))
            .tool(Box::new(ShellTool))
    })
    .await;

    println!("{}", result.format_output());
    std::process::exit(result.exit_code());
}
```

### Example 3: JSON Output for Programmatic Consumption

Use `output_format: "json"` when a downstream process needs to parse the result:

```rust
use echo_agent::headless::{HeadlessConfig, run_headless};

#[tokio::main]
async fn main() {
    let config = HeadlessConfig {
        prompt: "Summarize the changes in the last commit".into(),
        exit_on_error: true,
        output_format: "json".into(),   // ← JSON envelope
        max_iterations: Some(10),
    };

    let result = run_headless(config, |builder| builder).await;

    // Prints:
    // {
    //   "success": true,
    //   "model": "qwen3-max",
    //   "output": "..."
    // }
    println!("{}", result.format_output());
    std::process::exit(result.exit_code());
}
```

Consuming the output from a shell script:

```bash
OUTPUT=$(my-agent 2>/dev/null)
SUCCESS=$(echo "$OUTPUT" | jq -r '.success')
if [ "$SUCCESS" = "true" ]; then
    echo "$OUTPUT" | jq -r '.output'
else
    echo "Agent failed" >&2
    exit 1
fi
```

### Example 4: CI/CD Integration (GitHub Actions)

A typical GitHub Actions workflow that runs the agent on every pull request:

```yaml
name: AI Code Review
on:
  pull_request:
    branches: [main]

jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Build agent
        run: cargo build --release

      - name: Run headless review
        env:
          LLM_API_KEY: ${{ secrets.LLM_API_KEY }}
        run: |
          ./target/release/my-agent \
            --prompt "Review the diff in this PR for bugs and style issues" \
            --output-format json \
            --max-iterations 15 \
          | tee review.json

      - name: Post review comment
        if: always()
        run: |
          COMMENT=$(jq -r '.output' review.json)
          gh pr comment ${{ github.event.pull_request.number }} --body "$COMMENT"
```

Key points for CI/CD:
- **`max_iterations`** — always set a cap to prevent runaway builds
- **`exit_code()`** — `std::process::exit(result.exit_code())` fails the CI step on agent errors
- **`output_format: "json"`** — makes it easy for downstream steps to parse and act on the result

---

## Error Handling

`run_headless` never panics. All failures are captured in `HeadlessResult`:

| Scenario | `success` | `output` |
|---|---|---|
| Empty prompt | `false` | `"Error: empty prompt"` |
| Agent build failure | `false` | `"Error building agent: <detail>"` |
| Agent execution error | `false` | `"Error: <detail>"` |
| Successful execution | `true` | Agent's final answer |

This means callers can always rely on `exit_code()` and `format_output()` without needing a separate `match` on `Result`.

---

## Comparison with Other Modes

| | Headless | Chat / REPL | Streaming |
|---|---|---|---|
| Interaction | None | Interactive | Interactive |
| Prompts | Single | Multi-turn | Multi-turn |
| Output | Collected, returned | Printed live | Token-by-token |
| Exit behavior | Process exits | Stays alive | Stays alive |
| Best for | CI/CD, scripts | Development | UX-sensitive apps |

---

## See Also

- [React Agent](01-react-agent.md) — the underlying agent loop
- [Streaming Output](10-streaming.md) — real-time token delivery for interactive use
- [Long-Running Tasks](29-long-running-tasks.md) — background task execution within a session
