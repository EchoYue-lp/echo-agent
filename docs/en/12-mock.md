# Mock Testing Infrastructure

## What It Is

The `echo_agent::testing` module provides a suite of tools for testing components at every layer **without making any real LLM calls or network requests**.

| Type | Replaces | Typical Use |
|------|----------|-------------|
| `MockLlmClient` | Real LLM (OpenAI, etc.) | Test `SummaryCompressor` and any component that depends on `LlmClient` |
| `MockTool` | Real tools (databases, HTTP APIs, etc.) | Test tool parameter parsing, error handling |
| `MockAgent` | Real SubAgent | Test multi-agent orchestration logic |
| `FailingMockAgent` | An always-failing SubAgent | Test orchestration fault-tolerance paths |

Combined with the built-in `InMemoryStore` and `InMemoryCheckpointer`, these cover the vast majority of unit and integration test scenarios.

---

## Problem It Solves

### Challenges of testing LLM-dependent code

Real LLM calls have serious testing problems:

- **Unreliable**: network issues and API rate limits cause test failures unrelated to your code
- **Unpredictable**: the same input produces different outputs each time — assertions are fragile
- **Slow**: a single API call typically takes several seconds
- **Costly**: token-based billing makes frequent CI/CD runs expensive
- **Requires credentials**: complex setup in test environments; a barrier for open-source contributors

### What mocks solve

- **Zero network requests**: tests run entirely in memory, completing in milliseconds
- **Fully controlled**: precisely prescribe what each call returns
- **Observable**: verify that the component actually made the right calls (count, arguments)
- **Error injection**: easily simulate network failures, rate limiting, service outages

---

## MockLlmClient

Implements the `LlmClient` trait. Use it to test components that accept `Arc<dyn LlmClient>` as a dependency (e.g. `SummaryCompressor`).

### Basic usage

```rust
use echo_agent::testing::MockLlmClient;
use echo_agent::compression::compressor::{SummaryCompressor, DefaultSummaryPrompt};
use std::sync::Arc;

// Create mock with a scripted response queue
let mock_llm = Arc::new(
    MockLlmClient::new()
        .with_response("Summary: user asked about the weather.")
        .with_response("Summary: user asked for more details.")
);

// Inject into the compressor
let compressor = SummaryCompressor::new(mock_llm.clone(), DefaultSummaryPrompt, 2);

// ... run compression ...

// Post-run assertions
assert_eq!(mock_llm.call_count(), 1);  // LLM was called exactly once
let sent = mock_llm.last_messages().unwrap();
println!("LLM received {} messages", sent.len());
```

### Error injection

```rust
use echo_agent::testing::MockLlmClient;
use echo_agent::error::{ReactError, LlmError};

let mock = MockLlmClient::new()
    .with_response("Normal response")
    .with_network_error("Simulated timeout")   // convenience method
    .with_rate_limit_error()                   // 429 Too Many Requests
    .with_error(ReactError::Llm(LlmError::EmptyResponse)); // custom error

// Call 1 → "Normal response"
// Call 2 → Err(NetworkError)
// Call 3 → Err(ApiError { status: 429 })
// Call 4 → Err(EmptyResponse)
```

### API reference

| Method | Description |
|--------|-------------|
| `with_response(text)` | Enqueue a successful response |
| `with_responses(iter)` | Enqueue multiple successful responses |
| `with_error(err)` | Enqueue an error response |
| `with_network_error(msg)` | Enqueue a network error (convenience) |
| `with_rate_limit_error()` | Enqueue a 429 rate limit error |
| `call_count()` | Number of calls made so far |
| `last_messages()` | Messages sent in the most recent call |
| `all_calls()` | All call message lists in chronological order |
| `remaining()` | Responses remaining in the queue |
| `reset_calls()` | Clear call history |

---

## MockTool

Implements the `Tool` trait. Use it to test Agent tool-call behavior without relying on external services.

### Basic usage

```rust
use echo_agent::testing::MockTool;
use echo_agent::tools::Tool;
use std::collections::HashMap;

let tool = MockTool::new("database_query")
    .with_description("Query the database")
    .with_response(r#"[{"id":1,"name":"Alice"},{"id":2,"name":"Bob"}]"#)
    .with_failure("Database connection timed out");

// First execution → success
let r1 = tool.execute(HashMap::new()).await?;
assert!(r1.success);

// Second execution → failure
let r2 = tool.execute(HashMap::new()).await?;
assert!(!r2.success);

assert_eq!(tool.call_count(), 2);
```

### Asserting on input parameters

```rust
let mut params = HashMap::new();
params.insert("city".to_string(), serde_json::json!("Seattle"));

tool.execute(params).await?;

let last = tool.last_args().unwrap();
assert_eq!(last["city"], "Seattle");
```

### API reference

| Method | Description |
|--------|-------------|
| `new(name)` | Create a named mock tool |
| `with_description(desc)` | Set the tool description |
| `with_parameters(schema)` | Set the parameters JSON Schema |
| `with_response(text)` | Enqueue a success response |
| `with_responses(iter)` | Enqueue multiple success responses |
| `with_failure(msg)` | Enqueue a failure response |
| `call_count()` | Number of executions |
| `last_args()` | Arguments from the most recent call |
| `all_calls()` | All call argument maps in order |
| `reset_calls()` | Clear call history |

---

## MockAgent

Implements the `Agent` trait. Use it to replace real SubAgents when testing orchestration logic.

### Basic usage

```rust
use echo_agent::testing::MockAgent;
use echo_agent::agent::Agent;

let mut math_agent = MockAgent::new("math_agent")
    .with_response("6 × 7 = 42")
    .with_response("√144 = 12");

let r1 = math_agent.execute("Calculate 6 * 7").await?;
assert_eq!(r1, "6 × 7 = 42");

let r2 = math_agent.execute("Calculate √144").await?;
assert_eq!(r2, "√144 = 12");

assert_eq!(math_agent.call_count(), 2);
assert_eq!(math_agent.calls()[0], "Calculate 6 * 7");
```

### Combining with a real orchestrator

```rust
use echo_agent::prelude::*;
use echo_agent::testing::MockAgent;

let math  = MockAgent::new("math_agent").with_response("The answer is 42");
let writer = MockAgent::new("writer_agent").with_response("Report generated");

let config = AgentConfig::new("gpt-4o", "orchestrator", "Delegate to specialists")
    .role(AgentRole::Orchestrator)
    .enable_subagent(true);

let mut orchestrator = ReactAgent::new(config);
orchestrator.register_agent(Box::new(math));
orchestrator.register_agent(Box::new(writer));

// Orchestrator uses real LLM; SubAgents are mocked
let result = orchestrator.execute("Complete the task").await?;
```

### `FailingMockAgent` — testing fault tolerance

```rust
use echo_agent::testing::FailingMockAgent;

let mut broken = FailingMockAgent::new("broken_agent", "Downstream service unavailable");
let result = broken.execute("task").await;
assert!(result.is_err());
assert_eq!(broken.call_count(), 1); // failed calls are still recorded
```

### API reference (MockAgent)

| Method | Description |
|--------|-------------|
| `new(name)` | Create a named mock agent |
| `with_model(model)` | Set the model name |
| `with_system_prompt(prompt)` | Set the system prompt |
| `with_response(text)` | Enqueue a response |
| `with_responses(iter)` | Enqueue multiple responses |
| `call_count()` | Number of calls |
| `calls()` | All task strings in chronological order |
| `last_task()` | Task string from the most recent call |
| `reset_calls()` | Clear call history |

---

## Using InMemoryStore / InMemoryCheckpointer

For tests involving the memory system, use the built-in in-memory implementations (no file I/O):

```rust
use echo_agent::memory::checkpointer::{Checkpointer, InMemoryCheckpointer};
use echo_agent::memory::store::{InMemoryStore, Store};
use echo_agent::llm::types::Message;

// ── Store ──────────────────────────────────────────────────────
let store = InMemoryStore::new();
let ns = vec!["test_agent", "memories"];

store.put(&ns, "pref-001", serde_json::json!("user prefers dark mode")).await?;

let item = store.get(&ns, "pref-001").await?.unwrap();
assert_eq!(item.value, serde_json::json!("user prefers dark mode"));

let results = store.search(&ns, "dark", 10).await?;
assert_eq!(results.len(), 1);

// ── Checkpointer ───────────────────────────────────────────────
let cp = InMemoryCheckpointer::new();
let messages = vec![
    Message::user("Hello".to_string()),
    Message::assistant("Hi there!".to_string()),
];

cp.put("session-1", messages).await?;
let snapshot = cp.get("session-1").await?.unwrap();
assert_eq!(snapshot.messages.len(), 2);

cp.delete_session("session-1").await?;
assert!(cp.get("session-1").await?.is_none());
```

---

## Using in #[tokio::test]

The same mocks work directly in standard Rust tests:

```rust
#[cfg(test)]
mod tests {
    use echo_agent::compression::compressor::{SummaryCompressor, DefaultSummaryPrompt};
    use echo_agent::compression::{CompressionInput, ContextCompressor};
    use echo_agent::llm::types::Message;
    use echo_agent::testing::MockLlmClient;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_summary_compressor_calls_llm_once() {
        let mock = Arc::new(MockLlmClient::new().with_response("Summary text"));
        let compressor = SummaryCompressor::new(mock.clone(), DefaultSummaryPrompt, 2);

        let input = CompressionInput {
            messages: (0..6).flat_map(|i| vec![
                Message::user(format!("Q{i}")),
                Message::assistant(format!("A{i}")),
            ]).collect(),
            token_limit: 50,
            current_query: None,
        };

        let output = compressor.compress(input).await.unwrap();
        assert_eq!(mock.call_count(), 1);   // LLM called exactly once
        assert!(!output.messages.is_empty());
    }

    #[tokio::test]
    async fn test_summary_compressor_propagates_llm_error() {
        let mock = Arc::new(MockLlmClient::new().with_network_error("timeout"));
        let compressor = SummaryCompressor::new(mock, DefaultSummaryPrompt, 2);

        let input = CompressionInput {
            messages: vec![
                Message::user("hi".to_string()),
                Message::assistant("hello".to_string()),
                Message::user("bye".to_string()),
            ],
            token_limit: 10,
            current_query: None,
        };

        assert!(compressor.compress(input).await.is_err());
    }
}
```

---

## Coverage Map

| Test scenario | Recommended tool | Requires real LLM? |
|--------------|-----------------|-------------------|
| Tool parameter parsing | `MockTool` | No |
| Tool error handling | `MockTool::with_failure()` | No |
| Sliding-window compression | `SlidingWindowCompressor` directly | No |
| LLM summary compression | `MockLlmClient` + `SummaryCompressor` | No |
| SubAgent orchestration logic | `MockAgent` + real orchestrator | Yes (orchestrator) |
| Orchestration fault tolerance | `FailingMockAgent` | Yes (orchestrator) |
| Memory storage | `InMemoryStore` | No |
| Session restore | `InMemoryCheckpointer` | No |
| End-to-end Agent behavior | Real LLM | Yes |

---

## Full Example

See: `examples/demo16_testing.rs`

```bash
cargo run --example demo16_testing
```
