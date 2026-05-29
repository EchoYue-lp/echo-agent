# Hooks System

## Overview

Hooks allow custom behavior to be injected at key points in the agent lifecycle. There are three independent hook systems:

1. **Skills Hooks** — the main hook system with 20 events and 5 action types
2. **Task Hooks** — lifecycle callbacks for DAG task execution
3. **Subagent Hooks** — lifecycle callbacks for subagent dispatch

---

## Skills Hooks

The primary hook system. Hooks are configured in YAML (via `echo-agent.yaml` or SKILL.md frontmatter) and executed by the `HookExecutor`.

### Hook Events

| Event | When Fires | Can Modify |
|-------|-----------|------------|
| `PreToolUse` | Before tool execution | Input, permission (allow/block) |
| `PostToolUse` | After tool succeeds | Output, continuation |
| `PostToolUseFailure` | After tool fails | Error feedback |
| `PermissionRequest` | Permission dialog appears | Auto-approve/deny |
| `PermissionDenied` | Permission denied | Retry signal |
| `SessionStart` | Session begins or resumes | Context injection |
| `SessionEnd` | Session terminates | Cleanup |
| `Stop` | Agent finishes responding | Continue reason |
| `StopFailure` | Agent encounters unrecoverable error | Alert/recovery |
| `Notification` | Agent needs user attention | Permission shortcut |
| `UserPromptSubmit` | User submits prompt | Context injection, block |
| `PreCompact` | Before context compression | Context injection |
| `PostCompact` | After context compression | Context injection |
| `ConfigChange` | Configuration file changes | Block/reload |
| `InstructionsLoaded` | Skills/instructions loaded | Post-load validation |
| `PostToolBatch` | After batch of parallel tool calls | Aggregation |
| `SubagentStart` | Before subagent dispatch | Context injection |
| `SubagentStop` | After subagent completes | Result injection |
| `TaskCreated` | Task created/scheduled | Context injection |
| `TaskCompleted` | Task completed | Result injection |

### Hook Types

| Type | Behavior |
|------|----------|
| `command` | Execute a shell command; stdin receives JSON context |
| `prompt` | Inject a prompt message for the LLM |
| `permission` | Return a permission decision directly (allow/deny/ask) |
| `http` | POST event data to a URL, parse response |
| `mcp_tool` | Call an MCP server tool |

### YAML Configuration

```yaml
hooks:
  PreToolUse:
    - matcher: "Bash"
      hooks:
        - type: command
          command: "${SKILL_DIR}/validate.sh"
          timeout: 5
    - matcher: "Write"
      hooks:
        - type: prompt
          prompt: "Check file permissions before writing"
  PostToolUse:
    - matcher: "Edit|Write"
      hooks:
        - type: command
          command: "jq -r '.tool_input.file_path' | xargs prettier --write"
  Stop:
    - hooks:
        - type: command
          command: "osascript -e 'display notification \"Done\"'"
  SessionStart:
    - matcher: "startup"
      hooks:
        - type: prompt
          prompt: "Remember to use bun, not npm."
  PermissionRequest:
    - matcher: "shell"
      hooks:
        - type: permission
          decision: "allow"
```

### Matcher Patterns

The `matcher` field filters which tools/events trigger the hook:

- `"Bash"` — exact match on tool name
- `"Edit|Write"` — regex alternation (matches Edit or Write)
- `"*"` or omit matcher — matches all events
- `"startup"` — matches SessionStart with context keyword

### Command Hook Context

Command hooks receive JSON on stdin with the following structure:

```json
{
  "event": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": { "command": "ls -la" },
  "session_id": "abc123",
  "timestamp": "2026-05-29T10:30:00Z"
}
```

The command's stdout is parsed as a `HookResult`:

```json
{
  "decision": "allow",
  "modified_input": { "command": "ls -la --color=never" },
  "message": "Modified command to disable colors"
}
```

### Security Limits

| Limit | Value | Purpose |
|-------|-------|---------|
| Default timeout | 10 seconds | Prevents hanging hooks |
| Max timeout | 300 seconds | Hard upper bound |
| Max command length | 32 KB | Prevents abuse via malformed YAML |
| Sandbox execution | Optional | Hooks can run inside sandbox |

---

## Task Hooks

Lifecycle callbacks for DAG task execution. Implement the `TaskHooks` trait.

### Trait Definition

```rust
use async_trait::async_trait;
use echo_orchestration::tasks::{RetryDecision, TaskHookContext, TaskHooks};

struct LoggingHooks;

#[async_trait]
impl TaskHooks for LoggingHooks {
    async fn before_execute(&self, ctx: &TaskHookContext) {
        println!("Starting task: {}", ctx.task.subject);
    }

    async fn after_execute(&self, ctx: &TaskHookContext, result: &str) {
        println!("Completed: {} -> {}", ctx.task.subject, result);
    }

    async fn on_failure(&self, ctx: &TaskHookContext, error: &str) -> RetryDecision {
        if ctx.task.retry_count < ctx.task.max_retries {
            RetryDecision::Retry { delay_secs: 1 }
        } else {
            RetryDecision::Fail
        }
    }
}
```

### Hook Context

```rust
pub struct TaskHookContext {
    pub task: Task,           // The task being executed
    pub attempt: u32,         // Current attempt (1-based)
    pub executor: Option<String>, // Agent executing the task
}
```

### Retry Decisions

| Decision | Behavior |
|----------|----------|
| `Retry { delay_secs }` | Re-execute after delay |
| `Skip` | Skip task, continue DAG |
| `Fail` | Mark task as failed |

---

## Subagent Hooks

Lifecycle callbacks for subagent dispatch. Implement the `SubagentHooks` trait.

### Trait Definition

```rust
use async_trait::async_trait;
use echo_agent::subagent::{SubagentHooks, SubagentHookContext, SubagentRetryDecision, SubagentResult};

struct MySubagentHooks;

#[async_trait]
impl SubagentHooks for MySubagentHooks {
    async fn before_dispatch(&self, ctx: &SubagentHookContext) {
        println!("Dispatching to: {}", ctx.subagent_name);
    }

    async fn after_dispatch(&self, ctx: &SubagentHookContext, result: &SubagentResult) {
        println!("Completed: {}", ctx.subagent_name);
    }

    async fn on_failure(&self, ctx: &SubagentHookContext, error: &str) -> SubagentRetryDecision {
        SubagentRetryDecision::Retry { delay_secs: 2 }
    }
}
```

### Hook Context

```rust
pub struct SubagentHookContext {
    pub parent_agent: String,      // Parent agent name
    pub subagent_name: String,     // Subagent being dispatched
    pub execution_mode: ExecutionMode, // Sync/Fork/Teammate
    pub task: String,              // The task being dispatched
    pub attempt: u32,              // Current attempt (1-based)
}
```

### Retry Decisions

| Decision | Behavior |
|----------|----------|
| `Retry { delay_secs }` | Re-dispatch after delay |
| `Fail` | Propagate error to parent |
| `Delegate { alternative_agent }` | Dispatch to a different subagent |

---

## Combining Hook Systems

All three hook systems can be used simultaneously:

```rust
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .with_task_hooks(Arc::new(LoggingHooks))
    .with_subagent_hooks(Arc::new(MySubagentHooks))
    .build()?;
```

Skills hooks are configured via YAML and loaded automatically from `echo-agent.yaml` or SKILL.md files.
