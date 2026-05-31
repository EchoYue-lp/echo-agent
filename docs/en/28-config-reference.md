# Configuration Reference

## Overview

echo-agent provides two configuration approaches:

1. **Rust API** — `AgentConfig` + `ReactAgentBuilder` for programmatic configuration
2. **YAML file** — `echo-agent.yaml` for declarative configuration

---

## AgentConfig — Runtime Configuration

The core runtime config struct. Constructed via `AgentConfig::new()` and modified with builder methods.

### Required Parameters

```rust
let config = AgentConfig::new(model_name, agent_name, system_prompt);
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `model_name` | `&str` | LLM model identifier (e.g. `"qwen3-max"`) |
| `agent_name` | `&str` | Agent name for logging/identification |
| `system_prompt` | `&str` | System prompt defining agent role/capabilities |

### Preset Constructors

| Preset | Tools | Memory | Task | CoT | Use Case |
|--------|-------|--------|------|-----|----------|
| `AgentConfig::minimal(model, prompt)` | off | off | off | off | Simple LLM wrapper |
| `AgentConfig::standard(model, name, prompt)` | on | off | off | on | General-purpose agent |
| `AgentConfig::full_featured(model, name, prompt)` | on | on | on | on | Full-featured agent |

### All Fields

#### Core Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model_name` | `String` | *(required)* | LLM model identifier |
| `agent_name` | `String` | *(required)* | Agent name for logging |
| `system_prompt` | `String` | *(required)* | System prompt |
| `role` | `AgentRole` | `Worker` | `Orchestrator` or `Worker` |
| `max_iterations` | `usize` | `10` | Max reasoning steps per turn |
| `temperature` | `Option<f32>` | `None` (model default) | LLM temperature (0.0–2.0) |
| `max_tokens` | `Option<u32>` | `None` (model default) | Max generation tokens |

#### Feature Toggles

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enable_tool` | `bool` | `false` | Enable calling registered tools |
| `enable_task` | `bool` | `false` | Enable task planning tools |
| `enable_human_in_loop` | `bool` | `false` | Enable human-in-the-loop approval |
| `enable_subagent` | `bool` | `false` | Enable sub-agent dispatch |
| `enable_memory` | `bool` | `false` | Enable long-term memory tools |
| `enable_cot` | `bool` | `false` | Enable chain-of-thought prompting |

#### Tool Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allowed_tools` | `Vec<String>` | `[]` (all allowed) | Tool allowlist |
| `tool_error_feedback` | `bool` | `true` | Feed tool errors back to LLM |
| `force_read_before_edit` | `bool` | `false` | Require read before write/edit/delete |
| `plan_mode` | `bool` | `false` | Read-only tools only |
| `max_tool_output_tokens` | `Option<usize>` | `None` | Auto-truncate tool output exceeding limit |
| `tool_execution` | `ToolExecutionConfig` | *(see below)* | Tool execution settings |

#### Memory & Persistence

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `memory_path` | `String` | `"~/.echo-agent/store.json"` | Memory store file path |
| `session_id` | `Option<String>` | `None` | Checkpointer session ID |
| `conversation_id` | `Option<String>` | `None` | Conversation store ID |
| `checkpointer_path` | `String` | `"~/.echo-agent/checkpoints.json"` | Checkpointer file path |

#### Context & Compression

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `token_limit` | `usize` | `usize::MAX` | Context token limit |
| `compress_threshold_ratio` | `f64` | `0.2` | Trigger compression when available ratio falls below |
| `response_format` | `Option<ResponseFormat>` | `None` (text) | Structured output format |
| `auto_project_rules` | `bool` | `true` | Auto-load `.echo-agent/AGENT.md` |
| `working_dir` | `Option<PathBuf>` | `None` (cwd) | Working directory for project rules |

#### LLM Resilience

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `llm_max_retries` | `usize` | `3` | Max retries after LLM failure |
| `llm_retry_delay_ms` | `u64` | `500` | Initial retry delay (exponential backoff) |

#### Streaming & Callbacks

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `stream_buffer_size` | `usize` | `256` | Streaming channel buffer size |
| `callbacks` | `Vec<Arc<dyn AgentCallback>>` | `[]` | Event callbacks |

#### Advanced

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `_reasoning_effort` | `String` | `"medium"` | Reasoning effort: low/medium/high |
| `token_budget_config` | `TokenBudgetConfig` | *(see below)* | Context window budget |

### Builder Methods

All fields can be set via builder-pattern chain calls:

```rust
let config = AgentConfig::new("qwen3-max", "assistant", "You are helpful")
    .enable_tool()
    .enable_memory()
    .enable_cot()
    .max_iterations(20)
    .token_limit(100_000)
    .temperature(0.7)
    .tool_error_feedback(true)
    .force_read_before_edit(true)
    .build();
```

---

## ReactAgentBuilder — High-Level Builder

Higher-level builder that constructs a `ReactAgent`. Handles LLM client injection, tool registration, memory setup, guards, snapshots, and more.

### Basic Usage

```rust
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .name("my-agent")
    .system_prompt("You are a code assistant")
    .enable_tools()
    .enable_memory()
    .enable_cot()
    .build()?;
```

### Presets

| Preset | Description |
|--------|-------------|
| `ReactAgentBuilder::simple(model, prompt)` | No tools, minimal config |
| `ReactAgentBuilder::standard(model, name, prompt)` | Tools enabled |
| `ReactAgentBuilder::full_featured(model, name, prompt)` | Tools + memory + planning |

### LLM Configuration

```rust
// Option 1: Auto-create from model name
ReactAgentBuilder::new()
    .model("qwen3-max")

// Option 2: Explicit LLM client
ReactAgentBuilder::new()
    .llm_client(my_client)

// Option 3: With config
ReactAgentBuilder::new()
    .llm_config(LlmConfig { base_url, api_key, model })
```

### Tool Configuration

```rust
ReactAgentBuilder::new()
    .enable_tools()                    // enable tool calling
    .tool(Box::new(MyTool::new()))     // register single tool
    .tools(vec![...])                  // register multiple tools
```

### Feature Flags

```rust
ReactAgentBuilder::new()
    .enable_memory()                   // long-term memory
    .enable_planning()                 // DAG task planning
    .enable_human_in_loop()            // approval gate (requires "human-loop" feature)
    .enable_subagent()                 // sub-agent dispatch (requires "subagent" feature)
    .enable_cot()                      // chain-of-thought
    .disable_cot()                     // disable CoT
```

### Structured Output

```rust
// Auto-generate JSON Schema from Rust type
ReactAgentBuilder::new()
    .output_type::<MyResponse>()

// Explicit format
ReactAgentBuilder::new()
    .response_format(ResponseFormat::JsonSchema {
        json_schema: JsonSchemaSpec { name, schema, strict }
    })
```

### Guards & Permissions

```rust
ReactAgentBuilder::new()
    .guard(my_guard)                              // add input/output guard
    .guards(vec![guard1, guard2])                 // add multiple guards
    .with_content_guard(ContentGuardMode::Block)  // PII detection (requires "content-guard" feature)
    .permission_policy(my_policy)                 // custom permission policy
    .audit_logger(my_logger)                      // audit logging
```

### Persistence & Tracing

```rust
ReactAgentBuilder::new()
    .store(memory_store)                          // long-term memory store
    .with_memory_tools(store)                     // register remember/recall/forget tools
    .checkpointer(checkpointer, session_id)       // session persistence
    .session_id("sess_1")                         // session ID
    .conversation_id("conv_1")                    // conversation transcript ID
    .with_run_store(run_store)                    // execution tracing
```

### Snapshots & Circuit Breaker

```rust
ReactAgentBuilder::new()
    .snapshot_policy(SnapshotPolicy::EveryN(5))   // snapshot frequency
    .max_snapshots(20)                            // max retained snapshots
    .with_circuit_breaker(CircuitBreakerConfig {  // LLM failure protection
        failure_threshold: 5,
        success_threshold: 2,
        timeout: Duration::from_secs(60),
    })
```

### Build

```rust
// Build as ReactAgent
let agent: ReactAgent = builder.build()?;

// Build as boxed trait object
let agent: Box<dyn Agent> = builder.build_boxed()?;
```

### Validation Rules

- `model` must be non-empty
- `max_iterations` must be > 0
- `enable_subagent` requires `enable_builtin_tools` to be true

---

## ToolExecutionConfig

Controls individual tool execution behavior:

```rust
pub struct ToolExecutionConfig {
    pub timeout_ms: u64,             // default: 30_000 (30s)
    pub retry_on_fail: bool,         // default: false
    pub max_retries: u32,            // default: 2
    pub retry_delay_ms: u64,         // default: 200
    pub max_concurrency: Option<usize>,       // default: None
    pub max_read_concurrency: Option<usize>,  // default: Some(32)
}
```

---

## TokenBudgetConfig

Fine-grained context window budget management:

```rust
pub struct TokenBudgetConfig {
    pub total_window: Option<usize>,  // default: None (auto-detected from model)
    pub system_pct: f64,              // default: 0.10 (10%)
    pub tool_pct: f64,                // default: 0.05 (5%)
    pub output_pct: f64,              // default: 0.10 (10%)
    pub safety_pct: f64,              // default: 0.10 (10%)
    pub enabled: bool,                // default: true
}
```

With defaults, conversation history gets **65%** of the context window.

Auto-detected model window sizes:
- `claude` → 200K
- `gpt-4o` → 128K
- `qwen3` → 128K
- default → 128K

---

## ResponseFormat

```rust
pub enum ResponseFormat {
    Text,                                          // plain text (default)
    JsonObject,                                    // JSON object
    JsonSchema { json_schema: JsonSchemaSpec },    // JSON Schema constrained
}

pub struct JsonSchemaSpec {
    pub name: String,
    pub schema: serde_json::Value,
    pub strict: bool,  // default: true
}
```

---

## CircuitBreakerConfig

Protects against repeated LLM failures:

```rust
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,     // default: 5 (consecutive failures → Open)
    pub success_threshold: u32,     // default: 2 (consecutive successes → Closed)
    pub timeout: Duration,          // default: 60s (Open duration before HalfOpen)
}
```

---

## SnapshotPolicy

Controls when agent state snapshots are taken:

```rust
pub enum SnapshotPolicy {
    EveryIteration,  // snapshot after each ReAct iteration (default)
    EveryN(usize),   // snapshot every N iterations
    Manual,          // only on explicit call
}
```

---

## YAML Configuration

echo-agent supports declarative configuration via `echo-agent.yaml`.

### File Search Order

1. `$ECHO_AGENT_CONFIG` environment variable
2. `./echo-agent.yaml` (current directory)
3. `~/.echo-agent/config.yaml` (user home)
4. Built-in defaults

### Full Example

```yaml
model:
  name: "qwen-plus"
  temperature: 0.7
  max_tokens: 4096

agent:
  name: "echo-assistant"
  system_prompt: "You are an intelligent assistant"
  max_iterations: 10
  enable_tools: true
  enable_memory: true
  enable_human_in_loop: true
  memory_path: "~/.echo-agent/memory"
  tool_timeout_ms: 120000
  token_limit: 0
  compress_strategy: "sliding"
  compress_window: 20

mcp:
  config_path: "~/.echo-agent/mcp.yaml"

channels:
  feishu:
    enabled: false
    app_id: ""
    app_secret: ""
    mode: "long_poll"

server:
  host: "0.0.0.0"
  port: 3000
  max_body_bytes: 1048576

logging:
  level: "info"
```

### YAML Sections

| Section | Struct | Description |
|---------|--------|-------------|
| `model` | `ModelConfig` | LLM model name, temperature, max_tokens |
| `agent` | `AgentYamlConfig` | Agent behavior toggles and paths |
| `mcp` | `McpYamlConfig` | MCP config file path |
| `channels` | `ChannelsConfig` | IM channel integrations (QQ, Feishu) |
| `webhooks` | `WebhooksConfig` | Webhook endpoints |
| `hooks` | `HooksDefinition` | Lifecycle hook rules |
| `server` | `ServerConfig` | HTTP server host/port |
| `logging` | `LoggingConfig` | Log level |

### Environment Variable Overrides

| Env Var | Effect |
|---------|--------|
| `ECHO_AGENT_CONFIG` | Explicit config file path |
| `QQ_APP_ID` | Sets QQ channel app_id, auto-enables QQ |
| `QQ_CLIENT_SECRET` | Sets QQ channel client_secret |
| `FEISHU_APP_ID` | Sets Feishu channel app_id, auto-enables Feishu |
| `FEISHU_APP_SECRET` | Sets Feishu channel app_secret |
| `MCP_CONFIG_PATH` | Sets MCP config file path |

---

## Feature Flags

All features are opt-in. The `default` feature set is **empty**. The `full` meta-feature enables everything.

```toml
[dependencies]
echo_agent = { version = "0.2", features = ["mcp", "web", "shell"] }
```

### Core Features

| Feature | Description |
|---------|-------------|
| `subagent` | Sub-agent dispatch and TeamAgent |
| `mcp` | Model Context Protocol integration |
| `tasks` | Task planning and DAG scheduling |
| `self-reflection` | Self-reflection/evaluation loops |
| `human-loop` | WebSocket-based human-in-the-loop approval |
| `plan-execute` | Plan-and-execute agent mode |

### Tool Features

| Feature | Description |
|---------|-------------|
| `web` | Web search/fetch tools |
| `shell` | Shell command execution |
| `files` | File manipulation tools |
| `git` | Git tools |
| `database` | SQL database tools |
| `media` | PDF/Excel/Word/image tools |
| `data` | Polars data processing tools |
| `chart` | Chart generation tools |
| `research` | ArXiv, Semantic Scholar, PDF tools |
| `sandbox` | Sandboxed script execution |

### Infrastructure Features

| Feature | Description |
|---------|-------------|
| `sqlite` | SQLite-backed state storage |
| `telemetry` | OpenTelemetry tracing + metrics |
| `a2a` | Agent-to-Agent HTTP service |
| `channels` | IM channel integrations (QQ, Feishu) |
| `rag` | Retrieval-augmented generation |
| `semantic-memory` | Embedding-based semantic memory |
| `workflow` | Workflow DSL engine |
| `multimodal` | Multimodal input support |
| `content-guard` | PII detection/redaction |
| `eval` | Evaluation framework |
| `improve` | Self-improvement framework |
| `testing` | Testing utilities |

---

## TelemetryConfig

OpenTelemetry configuration:

```rust
pub struct TelemetryConfig {
    pub otlp_endpoint: String,    // default: "http://localhost:4317"
    pub service_name: String,     // default: "echo-agent"
    pub enable_console: bool,     // default: true
}
```

Requires the `telemetry` feature flag.

---

## Quick Reference

### Minimal Agent

```rust
let config = AgentConfig::minimal("qwen3-max", "Say hello");
let agent = ReactAgent::new(config);
```

### Standard Agent with Tools

```rust
let config = AgentConfig::standard("qwen3-max", "assistant", "You are helpful");
let mut agent = ReactAgent::new(config);
agent.add_tool(Box::new(MyTool::new()));
```

### Full-Featured Agent via Builder

```rust
let agent = ReactAgentBuilder::full_featured("qwen3-max", "assistant", "You are helpful")
    .tool(Box::new(FileTool::new()))
    .tool(Box::new(ShellTool::new()))
    .with_run_store(run_store)
    .guard(my_guard)
    .build()?;
```

### YAML-Based Configuration

```rust
let config = AppConfig::load()?;  // loads echo-agent.yaml
```
