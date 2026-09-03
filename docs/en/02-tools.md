# Tool System

## What It Is

Tools are the only mechanism through which an Agent interacts with the external world. The LLM learns about a tool's capabilities via JSON Schema, decides when to call it and with what parameters, and the framework handles the actual execution and returns the result back to the LLM.

## Problem It Solves

LLMs are pure text models — they cannot:
- Execute code or shell commands
- Query real-time data (weather, stocks, databases)
- Read or write files
- Call external APIs

The tool system provides a standardized bridge, enabling the LLM to drive any external capability through declarative invocation.

---

## Architecture

```
Tool trait                        ← unified interface all tools implement
    │
ToolManager                       ← registry + executor
    ├─ register(tool)
    ├─ execute_tool(name, params)  ← unified execution entry (timeout, retry, concurrency)
    └─ to_openai_tools()           ← serialize to OpenAI function-calling format

Built-in tools (builtin):
    ├─ final_answer               ← output final result (always registered)
    ├─ task_create / task_update  ← revisioned task-graph CRUD
    ├─ task_list                  ← read the committed graph revision
    ├─ agent_tool                 ← dispatch to a registered Subagent
    ├─ human_in_loop              ← request human text input
    ├─ remember / recall / forget ← long-term memory operations
    └─ think                      ← explicit CoT tool (superseded by CoT text approach)

Extension tools (ready to use):
    ├─ tools/files       ← file read/write (2 tools)
    ├─ tools/shell       ← shell command execution
    ├─ tools/web         ← web search + page fetch (feature: web)
    ├─ tools/media       ← PDF, Excel, Word, Image (feature: media)
    ├─ tools/data        ← Polars data analysis (13 tools, feature: data)
    ├─ tools/chart       ← chart generation (feature: chart)
    ├─ tools/rag         ← RAG index/search/chunk (feature: rag)
    ├─ tools/research    ← ArXiv, Semantic Scholar, PDF fetch, BibTeX (feature: research)
    ├─ tools/database    ← SQL query tools (feature: database)
    └─ tools/others      ← math, weather, etc.

Total: 67 registered tools across 26 feature categories.
```

---

## Tool Trait — Complete Definition

> **Trait signature changed in v0.2.0.** No longer uses `#[async_trait]`; `execute` returns `BoxFuture`.

```rust
pub trait Tool: Send + Sync {
    /// Stable tool identifier exposed to the model.
    fn name(&self) -> &str;

    /// Human-readable tool description.
    fn description(&self) -> &str;

    /// JSON Schema describing accepted parameters.
    fn parameters(&self) -> serde_json::Value;

    /// Execute the tool (core method, must implement).
    fn execute<'a>(
        &'a self,
        parameters: ToolParameters,
    ) -> BoxFuture<'a, Result<ToolResult>>;

    // ── Optional methods below — all have default implementations ──

    /// Stream tool execution, producing incremental ToolStreamEvents.
    fn execute_stream<'a>(
        &'a self,
        params: ToolParameters,
    ) -> BoxFuture<'a, Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>>;

    /// Whether this tool supports streaming execution (default: false).
    fn supports_streaming(&self) -> bool;

    /// Validate parameters before execution (default: Ok(())).
    fn validate_parameters<'a>(
        &'a self,
        params: &'a ToolParameters,
    ) -> BoxFuture<'a, Result<()>>;

    /// Permissions required to invoke this tool (default: empty).
    fn permissions(&self) -> Vec<ToolPermission>;

    /// Risk level of this tool (default: Standard).
    fn risk_level(&self) -> ToolRiskLevel;

    /// Human-readable capability declaration (default: derived from risk_level).
    fn capability_description(&self) -> &str;
}
```

### Method Summary

| Method | Required | Default | Purpose |
|--------|----------|---------|---------|
| `name()` | ✅ | — | Tool identifier |
| `description()` | ✅ | — | Description shown to LLM |
| `parameters()` | ✅ | — | JSON Schema parameter definition |
| `execute()` | ✅ | — | Core execution logic |
| `execute_stream()` | ❌ | Wraps `execute()` into single `Complete` event | Streaming progress output |
| `supports_streaming()` | ❌ | `false` | Declare streaming support |
| `validate_parameters()` | ❌ | `Ok(())` | Pre-execution parameter validation |
| `permissions()` | ❌ | `vec![]` | Declare required permissions |
| `risk_level()` | ❌ | `Standard` | Risk classification |
| `capability_description()` | ❌ | Derived from risk_level | Human-readable capability text |

---

## ToolResult and ToolResultKind

### ToolResult

```rust
pub struct ToolResult {
    pub kind: ToolResultKind,        // Result type classification
    pub success: bool,               // Whether the tool succeeded
    pub output: String,              // Text output
    pub error: Option<String>,       // Error message
    pub failure: Option<ToolFailure>, // Typed failure/recovery facts
    pub data: Option<Value>,         // Structured JSON data
    pub truncated: bool,             // Whether output was truncated
    pub mime_type: Option<String>,   // MIME type
    pub artifact: Option<ToolOutputArtifactRef>, // Complete spilled output
    pub metadata: HashMap<String, String>, // Key-value metadata
}
```

**Constructors:**

| Method | Purpose |
|--------|---------|
| `ToolResult::success(output)` | Successful text result |
| `ToolResult::success_json(data)` | Successful JSON result |
| `ToolResult::success_with_kind(kind, output)` | Typed success result |
| `ToolResult::error(msg)` | Failed result |
| `with_artifact(reference)` | Attach a complete typed output artifact |

**Builder pattern:**

```rust
ToolResult::success("output")
    .with_meta("file_path", "/tmp/result.csv")
    .with_mime_type("text/csv")
    .with_truncated(true)
```

### ToolResultKind

```rust
pub enum ToolResultKind {
    Text,                                    // Plain text
    Json,                                    // Structured JSON
    Image { mime_type: String },             // Image
    Table { columns: Vec<String>, rows: Vec<Vec<String>> }, // Tabular data
    Diff { unified_diff: String },           // Unified diff
    FileReference { path: String },          // File reference
    CommandOutput { exit_code: Option<i32> }, // Command output
    StructuredError { error_code: String },  // Structured error
}
```

Downstream consumers (CLI rendering, trace analysis, eval scoring) can use `kind` for type-aware handling without parsing the `output` string.
Complete spilled output is carried only by `artifact`; consumers must not
reconstruct its path, digest, size, or retention from `metadata`. Applications
may validate the typed reference against their own registered roots and
retention policy before exposing it.

---

## ToolStreamEvent (Streaming Tool Events)

```rust
pub enum ToolStreamEvent {
    /// Progress notification with optional percentage (0-100).
    Progress { message: String, percent: Option<u8> },
    /// Incremental partial output chunk.
    PartialOutput { chunk: String },
    /// Terminal event carrying the final ToolResult. Stream ends after this.
    Complete(ToolResult),
}
```

Implementing a streaming tool:

```rust
impl Tool for LongRunningTool {
    // ... name / description / parameters ...

    fn supports_streaming(&self) -> bool { true }

    fn execute_stream<'a>(
        &'a self,
        params: ToolParameters,
    ) -> BoxFuture<'a, Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>> {
        Box::pin(async move {
            let stream = async_stream::stream! {
                yield ToolStreamEvent::Progress {
                    message: "Starting...".into(),
                    percent: Some(0),
                };
                // ... intermediate steps ...
                yield ToolStreamEvent::PartialOutput {
                    chunk: "partial result...".into(),
                };
                yield ToolStreamEvent::Progress {
                    message: "Done".into(),
                    percent: Some(100),
                };
                yield ToolStreamEvent::Complete(
                    ToolResult::success("final result")
                );
            };
            Ok(Box::pin(stream))
        })
    }

    fn execute<'a>(
        &'a self,
        params: ToolParameters,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        // Non-streaming fallback
        Box::pin(async move { Ok(ToolResult::success("final result")) })
    }
}
```

---

## Subagent identity and uplink channel

When a tool runs inside a **dispatched Subagent**, `ToolContext` additionally
carries:

- `subagent_lineage: Option<SubagentLineage>` — an identity snapshot of the
  attempt (own role name/execution_id/run_id, parent agent, parent
  execution_id, `root/<child>/...` tree path, task_id/attempt/plan_revision).
  `None` for primary agent invocations. The framework stamps it at dispatch;
  caller-stamped lineage fields (e.g. from `agent_tool`) take precedence.
- `uplink: Option<SubagentUplinkFn>` — the uplink channel installed by the
  dispatcher. The built-in `subagent_message` tool (opt in via
  `register_subagent_message_tools()`) sends `report`/`escalate` to the
  parent or queue-only notes to sibling attempts; delivery never blocks the
  sender and returns a `SubagentUplinkReceipt`. The default sink delivers
  through the shared control plane and emits
  `SubagentEvent::UplinkReceived`; applications may install their own sink
  to own routing (journal, pause policy, ...). See ADR 0027 and
  `echo-agent-learning/examples/demo50_subagent_communication.rs`.

---

---

## Retaining Invocation Resources

Some application-owned resources must remain alive until already-started tool
work actually settles. Examples include owned concurrency permits, temporary
workspace owners, and external service leases. Attach them to the invocation as
opaque guards:

```rust
use echo_agent::agent::AgentInvocationContext;
use echo_agent::tools::InvocationResourceGuard;

let invocation = AgentInvocationContext {
    resource_guards: vec![InvocationResourceGuard::new(my_owned_lease)],
    ..AgentInvocationContext::default()
};
```

The framework clones these guards through `AgentRunSnapshot`,
`ExternalRunContext`, subagent dispatch, and `ToolContext`. A tool that starts
independent asynchronous or blocking work must clone
`context.resource_guards` into that owned task. Dropping the caller then does
not release the resource before the task settles.
The default value-scoped methods on `Agent` also retain the invocation inside
the returned stream, so third-party Agent implementations inherit this
lifetime behavior without overriding the context-aware methods.

The wrapped value is intentionally opaque: tools can retain guards but cannot
downcast or inspect application state through this API. Debug output reports
only guard type names and counts, never wrapped values.
When several resource types are present, `guard.retains::<MyLease>()` provides
an exact-type predicate for filtering without returning the wrapped value. A
guard built with `Arc<MyLease>` must be queried as `retains::<Arc<MyLease>>()`.

When multiple guards retain the same resource type, attach an immutable typed
descriptor and match it without exposing the descriptor value:

```rust
#[derive(PartialEq, Eq)]
struct LeaseIdentity {
    scope: &'static str,
    generation: u64,
}

let guard = InvocationResourceGuard::new_identified(
    my_owned_lease,
    LeaseIdentity { scope: "workspace", generation: 7 },
);

assert!(guard.matches_identity(&LeaseIdentity {
    scope: "workspace",
    generation: 7,
}));
```

`matches_identity` requires the exact descriptor type and equal value. Missing,
wrongly typed, or unequal identities return `false`. The API returns only a
boolean; identity values are absent from Debug output and cannot be retrieved
or downcast.

See [ADR 0005](../adr/0005-invocation-resource-lifetime.md) for the ownership
and propagation decision.

---

## ToolRiskLevel

```rust
pub enum ToolRiskLevel {
    ReadOnly,   // Read-only, no side effects
    Standard,   // Standard, limited side effects
    Dangerous,  // Dangerous, irreversible side effects
}
```

Tools declare risk via `risk_level()`:

```rust
impl Tool for DeleteFileTool {
    fn risk_level(&self) -> ToolRiskLevel { ToolRiskLevel::Dangerous }
    fn capability_description(&self) -> &str { "Delete files — irreversible" }
    // ...
}
```

`ToolRiskClassifier` (in `echo-execution`) auto-classifies tools by name into 7 risk categories:

| Category | Risk Level | Example Tools |
|----------|-----------|---------------|
| `ReadOnly` | 0 | `read_file`, `grep`, `git_status` |
| `NetworkCall` | 1 | `web_fetch`, `web_search` |
| `FileWrite` | 2 | `apply_patch` |
| `GitWrite` | 2 | `git_commit`, `git_push` |
| `DatabaseWrite` | 2 | `db_execute`, `sql` |
| `ShellExec` | 3 | `shell`, `execute` |
| `Destructive` | 3 | `delete_file`, `drop_table` |

See [Security and Permissions](./security.md) for the full permission model, rule engine, and risk classification.

---

## ToolCallParams (Type-Safe Parameters)

Raw JSON parameters from the LLM can be extracted and validated type-safely via `ToolCallParams`:

```rust
use echo_agent::tools::{ToolCallParams, ParamValue};

let params = ToolCallParams::from_value(&raw_json);

// Type-safe extraction
let path: Option<&str> = params.get_str("path");
let count: Option<f64> = params.get_number("count");
let force: Option<bool> = params.get_bool("force");

// Required parameter validation
params.validate_required("path", "string")?;
```

---

## Implementing a Custom Tool

Implement the `Tool` trait (note: no longer uses `#[async_trait]`; `execute` returns `BoxFuture`):

```rust
use echo_agent::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};
use echo_agent::tools::permission::ToolPermission;
use echo_agent::error::Result;
use echo_agent::tools::ToolCallParams;
use serde_json::{Value, json};
use futures::future::BoxFuture;

struct TranslateTool;

impl Tool for TranslateTool {
    fn name(&self) -> &str {
        "translate"
    }

    fn description(&self) -> &str {
        "Translate text into a target language"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text":   { "type": "string", "description": "Text to translate" },
                "target": { "type": "string", "description": "Target language code, e.g. 'en', 'zh', 'ja'" }
            },
            "required": ["text", "target"]
        })
    }

    fn execute<'a>(
        &'a self,
        params: ToolParameters,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let typed = ToolCallParams::from_params(&params);
            let text = typed.get_str("text").unwrap_or("");
            let target = typed.get_str("target").unwrap_or("en");
            // Call actual translation API ...
            let result = format!("(Translated to {}) {}", target, text);
            Ok(ToolResult::success(result))
        })
    }

    // ── Optional: declare permissions and risk ──

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]  // Requires network access
    }

    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly  // Read-only operation
    }

    // ── Optional: parameter validation ──

    fn validate_parameters<'a>(
        &'a self,
        params: &'a ToolParameters,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let typed = ToolCallParams::from_params(params);
            typed.validate_required("text", "string")
                .map_err(|e| echo_agent::error::ReactError::Other(e))?;
            typed.validate_required("target", "string")
                .map_err(|e| echo_agent::error::ReactError::Other(e))?;
            Ok(())
        })
    }
}
```

---

## Registering and Using Tools

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new("qwen3-max", "agent", "You are a translation assistant")
    .enable_tool(true);

let mut agent = ReactAgent::new(config);
agent.add_tool(Box::new(TranslateTool));
// or bulk-register: agent.add_tools(vec![...]);

let answer = agent.execute("Translate 'Hello World' to Japanese").await?;
```

---

## Execution Config (timeout / retry / concurrency)

`ToolExecutionConfig` controls execution behavior for all tools:

```rust
use echo_agent::tools::ToolExecutionConfig;

let exec_config = ToolExecutionConfig {
    timeout_ms:      5_000,   // per-call timeout 5s (0 = unlimited)
    retry_on_fail:   true,    // auto-retry on failure
    max_retries:     2,       // max 2 retries
    retry_delay_ms:  300,     // first retry delay 300ms, exponential backoff
    max_concurrency: Some(3), // max 3 concurrent tool calls
};

let config = AgentConfig::new("qwen3-max", "agent", "...")
    .tool_execution(exec_config);
```

**Exponential backoff**: retry 1 → 300ms, retry 2 → 600ms, retry 3 → 1200ms...

---

## Restricting Tools with Allowlist

Use `allowed_tools` to limit which tools a given Agent can call. Commonly used to enforce capability boundaries on Subagents:

```rust
use echo_agent::tools::others::math::{AddTool, SubtractTool};

let config = AgentConfig::new("qwen3-max", "math_only", "Only do addition and subtraction")
    .allowed_tools(vec!["add".to_string(), "subtract".to_string()]);

let mut agent = ReactAgent::new(config);
// Even if more tools are registered, only 'add' and 'subtract' are exposed to the LLM
agent.add_tools(vec![
    Box::new(AddTool),
    Box::new(SubtractTool),
]);
```

---

## Built-in Tool Reference

| Tool Name | Module | Description |
|-----------|--------|-------------|
| `final_answer` | builtin | Output final result (auto-registered) |
| `task_create` | builtin | Atomically create or append to a revisioned task graph |
| `task_update` | builtin | Apply an optimistic graph patch |
| `task_list` | builtin | Read the committed task graph revision |
| `agent_tool` | builtin | Dispatch task to a Subagent |
| `human_in_loop` | builtin | Request human text input |
| `remember` | builtin | Write a memory to Store |
| `recall` | builtin | Search memories in Store |
| `forget` | builtin | Delete a memory from Store |
| `read_file` | files | Read file contents |
| `write_file` | files | Write file contents |
| `shell` | shell | Execute shell command |
| `add` / `subtract` / ... | others | Math operations (examples) |
| `get_weather` | others | Weather query (example) |
| `web_search` | web | Web search (requires `web` feature) |
| `web_fetch` | web | Fetch web page and convert to text (requires `web` feature) |
| `arxiv_search` | research | Search ArXiv for academic papers (requires `research` feature) |
| `semantic_scholar_search` | research | Search Semantic Scholar (requires `research` feature) |
| `pdf_fetch` | research | Download and parse PDF from URL (requires `research` feature) |
| `bibtex_generate` | research | Generate BibTeX from paper metadata (requires `research` feature) |
| `rag_index` | rag | Chunk and vector-index documents (requires `rag` feature) |
| `rag_search` | rag | Semantic search over indexed documents (requires `rag` feature) |
| `excel_read` / `excel_write` / ... | media | Excel read/write/profile (6 tools, requires `media` feature) |
| `data_read` / `data_filter` / ... | data | Polars data analysis (13 tools, requires `data` feature) |
| `generate_chart` | chart | Chart generation (requires `chart` feature) |
| `db_query` / `db_schema` | database | SQL database tools (requires `database` feature) |

See: `echo-agent-learning/examples/demo01_tools.rs`, `echo-agent-learning/examples/demo09_file_shell.rs`, `echo-agent-learning/examples/demo13_tool_execution.rs`, `echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs`

---

## ToolChoice Enum (v0.2.1)

Type-safe enum controlling how the LLM uses tools:

| Variant | Meaning | OpenAI Format |
|---------|---------|---------------|
| `Auto` | Model decides whether to call tools (default) | `"auto"` |
| `None` | Prevent any tool calls | `"none"` |
| `Required` | Model must call at least one tool | `"required"` |
| `Function { name }` | Force call to a specific tool | `{"type":"function","function":{"name":"..."}}` |

```rust
use echo_agent::llm::ToolChoice;

// Let the model decide
let choice = ToolChoice::Auto;

// Force a specific tool
let choice = ToolChoice::function("web_search");

// Disable tool calls
let choice = ToolChoice::None;
```

---

## Tool Execution Pipeline (ToolExecutionPipeline)

> **New in v0.2.0.** Configurable multi-stage tool execution pipeline.

Tool calls no longer execute directly — they flow through a pluggable pipeline. Each stage can inspect, modify, intercept, or augment tool execution behavior.

### Pipeline Stages

```
Tool Call → InterventionStage → ParseValidate → PlanMode → PreToolUseHook
           → Permission → ReadBeforeEdit → Callback(Start) → Execution
           → TraceRecording → PostToolUseHook → OutputGuard → Truncation
           → Callback(End)
```

| Stage | Purpose |
|-------|---------|
| **InterventionStage** | Intervention callbacks: block / cancel / redirect / modify_args |
| **ParseValidate** | Parameter parsing and type validation |
| **PlanMode** | Intercept write operations in planning mode |
| **PreToolUseHook** | PreToolUse hooks: can modify input or block execution |
| **Permission** | Permission check (PermissionService unified pipeline) |
| **ReadBeforeEdit** | Force file read before edit (prevents blind writes) |
| **Callback(Start)** | on_tool_start callbacks |
| **Execution** | Actual tool execution |
| **TraceRecording** | Record trace events |
| **PostToolUseHook** | PostToolUse hooks |
| **OutputGuard** | Output content guard check |
| **Truncation** | Output truncation (token budget) |
| **Callback(End)** | on_tool_end callbacks |

### Configuring the Pipeline

```rust
use echo_agent::agent::ToolExecutionPipeline;
use echo_agent::prelude::*;

let pipeline = ToolExecutionPipeline::default();

let agent = ReactAgentBuilder::new()
    .tool_execution_pipeline(pipeline)
    .build(config);
```

See [demo64_tool_pipeline.rs](../../echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs).
