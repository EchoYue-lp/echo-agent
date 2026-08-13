# Hooks System

## Overview

Hooks allow custom behavior to be injected at key points in the agent lifecycle. There are three independent hook systems:

1. **Skills Hooks** — the main hook system with 31 events and 7 action types
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
| `TaskStarted` | Scheduler claims a task attempt | Context injection |
| `TaskCompleted` | Task completed | Result injection |
| `PluginLoaded` | Plugin components become live | Notification/context |
| `PluginDisabled` | Plugin is disabled or uninstalled | Notification/context |
| `PostMemoryWrite` | Memory is persisted | Evolution feedback |
| `MemoryLayerChange` | Memory changes layer | Evolution feedback |
| `SkillCandidateDetected` | A skill candidate is detected | Evolution feedback |
| `SkillLifecycleTransition` | A skill changes lifecycle state | Evolution feedback |
| `SkillHealthCheck` | A skill health check finishes | Evolution feedback |
| `SkillPatchApplied` | A skill patch is applied | Evolution feedback |
| `SkillMergeApplied` | Skills are merged | Evolution feedback |
| `RulePromoted` | Memory is promoted to an AGENTS.md rule | Evolution feedback |

All eight Evolution events above are emitted by their owning write, transition,
candidate-detection, health-check, patch, merge, and rule-promotion paths. They
are runtime events rather than reserved enum values.

### Hook Types

| Type | Behavior |
|------|----------|
| `command` | Execute a shell command; stdin receives JSON context |
| `prompt` | Inject a prompt message for the LLM |
| `permission` | Return a permission decision directly (allow/deny/ask) |
| `http` | POST event data to a URL, parse response |
| `mcp_tool` | Call an MCP server tool |
| `agent` | Dispatch a named subagent for the hook action |
| `activate_skill` | Activate a discovered skill directly, without an LLM round trip |

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
- `"Edit|Write"` — pipe-separated alternatives (matches Edit or Write)
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
  "updatedInput": { "command": "ls -la --color=never" },
  "injected_context": "Modified command to disable colors",
  "permission_mode_override": "auto"
}
```

These are the canonical wire names; `modified_input`, `message`, and
`permission_mode` are not aliases. `permission_mode_override` may be returned
by `PreToolUse` or `PermissionRequest`. It applies only to that tool call and is
passed into the permission service without mutating the session's configured
mode. Canonical values are `default`, `plan`, `auto`, `acceptEdits`,
`bypassPermissions`, `bubble`, `dontAsk`, and `strict`.

### Sources, Reloading, and Dry Run

User, Skill, and Plugin hooks all use the same registration-time action
validation. Invalid actions are logged and omitted while valid actions in the
same rule remain active.

EKO merges inline hooks from `echo-agent.yaml`, global
`~/.eko/hooks.yaml`, and project `.eko/hooks.yaml`. Its watcher monitors all
three targets. Create, modify, atomic-replace, and remove events all trigger a
reload, so deleting a `hooks.yaml` removes its registered hooks without a
restart. A failed parse keeps the last known good registry. CLI, TUI, and GUI hook tests call
`HookRegistry::dry_run`: they evaluate event and matcher routing and report the
source/action list without executing side effects.

### Runtime Limits and Local Extension Policy

| Limit | Value | Purpose |
|-------|-------|---------|
| Default timeout | 10 seconds | Prevents hanging hooks |
| Max timeout | 300 seconds | Hard upper bound |
| Max command length | 32 KB | Prevents abuse via malformed YAML |
| Sandbox execution | Optional | Hooks can run inside sandbox |

EKO is a local, user-controlled application. Hook HTTP actions therefore allow
plain HTTP for loopback, private-network, and link-local IP literals, as well as
`localhost`, single-label hosts such as `nas`, and names ending in `.local` or
`.lan`. Remote addresses must use HTTPS. Configured headers and payloads are
sent unchanged, while sensitive substituted values are redacted from command
diagnostics. MCP actions may invoke any tool exposed by the user-configured
server; there is no framework deny-list for locally trusted extensions.

---

## Task Hooks

Lifecycle callbacks for DAG task execution. Implement the `TaskHooks` trait.

### Trait Definition

```rust
use async_trait::async_trait;
use echo_agent::tasks::{RetryDecision, TaskHookContext, TaskHooks};

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
