# Tool System

## What It Is

Tools are the only mechanism through which an Agent interacts with the external world. The LLM learns about a tool's capabilities via JSON Schema, decides when to call it and with what parameters, and the framework handles the actual execution and returns the result back to the LLM.

---

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
    ├─ plan                       ← trigger planning mode (Planner role)
    ├─ create_task / update_task  ← manage DAG sub-tasks
    ├─ agent_tool                 ← dispatch to SubAgent (Orchestrator role)
    ├─ human_in_loop              ← request human text input
    ├─ remember / recall / forget ← long-term memory operations
    └─ think                      ← explicit CoT tool (superseded by CoT text approach)

Extension tools (ready to use):
    ├─ tools/files    ← file read/write
    ├─ tools/shell    ← shell command execution
    └─ tools/others   ← math, weather, etc.
```

---

## Implementing a Custom Tool

Implement the `Tool` trait:

```rust
use echo_agent::tools::{Tool, ToolParameters, ToolResult};
use echo_agent::error::Result;
use serde_json::{Value, json};
use async_trait::async_trait;

struct TranslateTool;

#[async_trait]
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

    async fn execute(&self, params: ToolParameters) -> Result<ToolResult> {
        let text   = params["text"].as_str().unwrap_or("");
        let target = params["target"].as_str().unwrap_or("en");
        // Call actual translation API ...
        let result = format!("(Translated to {}) {}", target, text);
        Ok(ToolResult::success(result))
    }
}
```

---

## Registering and Using Tools

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new("gpt-4o", "agent", "You are a translation assistant")
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

let config = AgentConfig::new("gpt-4o", "agent", "...")
    .tool_execution(exec_config);
```

**Exponential backoff**: retry 1 → 300ms, retry 2 → 600ms, retry 3 → 1200ms...

---

## Restricting Tools with Allowlist

Use `allowed_tools` to limit which tools a given Agent can call. Commonly used to enforce capability boundaries on SubAgents:

```rust
use echo_agent::tools::others::math::{AddTool, SubtractTool};

let config = AgentConfig::new("gpt-4o", "math_only", "Only do addition and subtraction")
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
| `plan` | builtin | Trigger task planning (Planner mode) |
| `create_task` | builtin | Create a DAG sub-task |
| `update_task` | builtin | Update sub-task status |
| `list_tasks` | builtin | List all sub-tasks |
| `agent_tool` | builtin | Dispatch task to a SubAgent |
| `human_in_loop` | builtin | Request human text input |
| `remember` | builtin | Write a memory to Store |
| `recall` | builtin | Search memories in Store |
| `forget` | builtin | Delete a memory from Store |
| `read_file` | files | Read file contents |
| `write_file` | files | Write file contents |
| `shell` | shell | Execute shell command |
| `add` / `subtract` / ... | others | Math operations (examples) |
| `get_weather` | others | Weather query (example) |

See: `examples/demo01_tools.rs`, `examples/demo09_file_shell.rs`, `examples/demo13_tool_execution.rs`
