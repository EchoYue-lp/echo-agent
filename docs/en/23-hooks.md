# Hooks System

## Overview

Hooks allow custom behavior to be injected at key points in the agent lifecycle. There are three independent hook systems:

1. **Skills Hooks** — the main hook system with 31 events and 7 action types
2. **Task Hooks** — lifecycle callbacks for DAG task execution
3. **Subagent Hooks** — lifecycle callbacks for subagent dispatch

---

## Skills Hooks

The primary hook system. Hooks are configured in YAML via host application or
plugin configuration and executed by the `HookExecutor`.

`HookEvent::ALL` is a 31-event catalog, not a promise that every event is
automatically emitted by a bare `ReactAgent`. The producer contract is explicit:
`framework-auto` means a framework-owned runtime path emits the event when that
path is used; `host-owned` means the embedding application must invoke an
event-specific bridge or opt-in observer; `no-producer` means the enum and context
support the event, but the current framework has no producer. The complete
matrix is also recorded in [ADR 0073](../adr/0073-hook-event-producer-contract.md)
and locked by `tests/hook_event_producer_contract.rs`.

### Hook event producer matrix

| Event | Category | Producer status | Current producer / owner |
|-------|----------|-----------------|-------------------------|
| `PreToolUse` | Tool | `framework-auto` | ReAct tool pipeline before execution |
| `PostToolUse` | Tool | `framework-auto` | ReAct tool pipeline after success |
| `PostToolUseFailure` | Tool | `framework-auto` | ReAct tool pipeline after failure |
| `PermissionRequest` | Tool | `framework-auto` | ReAct permission stage |
| `PermissionDenied` | Tool | `no-producer` | Reserved for the approval-receipt work (#37); do not synthesize it through the generic lifecycle API |
| `SessionStart` | Session/run | `framework-auto` | Runtime reset/restore path |
| `SessionEnd` | Session/run | `framework-auto` | ReAct finalization and agent close paths |
| `Stop` | Session/run | `framework-auto` | ReAct final answer/intervention path |
| `Notification` | Session/run | `no-producer` | No dedicated framework or host adapter; the generic lifecycle API is only a manual escape hatch |
| `UserPromptSubmit` | Session/run | `framework-auto` | ReAct context preparation |
| `PreCompact` | Session/run | `framework-auto` | Framework compression entry points |
| `PostCompact` | Session/run | `framework-auto` | Framework compression completion path |
| `ConfigChange` | Session/run | `no-producer` | No dedicated framework or host adapter; the generic lifecycle API is only a manual escape hatch |
| `InstructionsLoaded` | Session/run | `framework-auto` | `ReactAgent::discover_skills` after registration |
| `PostToolBatch` | Session/run | `framework-auto` | ReAct parallel-tool batch settlement |
| `SubagentStart` | Subagent | `framework-auto` | `SubagentExecutor::unified_hook_executor` on each concrete dispatch attempt |
| `SubagentStop` | Subagent | `framework-auto` | `SubagentExecutor::unified_hook_executor` at each attempt terminal boundary |
| `TaskCreated` | Task | `host-owned` | Application-owned task runtime through `TaskHookBridge` |
| `TaskStarted` | Task | `host-owned` | Application-owned task runtime through `TaskHookBridge` |
| `TaskCompleted` | Task | `host-owned` | Application-owned task runtime through `TaskHookBridge` |
| `StopFailure` | Error | `framework-auto` | ReAct terminal failure paths |
| `PluginLoaded` | Plugin | `framework-auto` | `PluginCoordinator` event-emission phase |
| `PluginDisabled` | Plugin | `framework-auto` | `PluginCoordinator` event-emission phase |
| `PostMemoryWrite` | Evolution | `host-owned` | Opt-in `HookEvolutionObserver` wired by the embedding application |
| `MemoryLayerChange` | Evolution | `host-owned` | Opt-in `HookEvolutionObserver` wired by the embedding application |
| `SkillCandidateDetected` | Evolution | `host-owned` | Opt-in `HookEvolutionObserver` wired by the embedding application |
| `SkillLifecycleTransition` | Evolution | `no-producer` | Enum/context support only; no framework observer callback exists |
| `SkillHealthCheck` | Evolution | `host-owned` | Opt-in `HookEvolutionObserver` wired by the embedding application |
| `SkillPatchApplied` | Evolution | `no-producer` | Enum/context support only; no framework observer callback exists |
| `SkillMergeApplied` | Evolution | `no-producer` | Enum/context support only; no framework observer callback exists |
| `RulePromoted` | Evolution | `no-producer` | Enum/context support only; no framework observer callback exists |

The `host-owned` evolution rows are real only after the host wires
`HookEvolutionObserver` into the relevant runtime. The four `no-producer`
evolution rows must not be described as automatic callbacks. Adding a producer
for any `no-producer` event is a separate contract change that must update this
matrix and the contract test.

`SubagentExecutor::unified_hook_executor` is the canonical producer for the
default `ReactAgent` path. `SubagentHookBridge` remains an explicit adapter for
hosts that own a separate Subagent runtime; a host must choose one path per
dispatch attempt rather than wiring both, or it will duplicate Start/Stop.

`MemoryLayerChange` also reports a successful hot-memory deletion as
`from_layer = "hot"` and `to_layer = "deleted"`. Missing or failed deletes do
not emit the event.

### Hook Types

| Type | Behavior |
|------|----------|
| `command` | Execute a shell command; stdin receives JSON context |
| `prompt` | Inject a prompt message for the LLM |
| `permission` | Return a permission decision directly (allow/deny/ask) |
| `http` | POST event data to a URL, parse response |
| `mcp_tool` | Call an MCP server tool |
| `subagent` | Dispatch a named subagent through the agent's registered Subagent runtime |
| `activate_skill` | Activate a discovered skill directly, without an LLM round trip |

### Configuration

Hooks are configured by the host application and plugins. The official
agentskills.io Skill file format has no per-skill Hook field or sidecar;
application configuration uses the following YAML mapping:

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
  StopFailure:
    - hooks:
        - type: subagent
          name: incident-reviewer
          task: "Summarize the failure and propose recovery steps"
          timeout: 900
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

Plugin command hooks also receive `PLUGIN_ROOT` and `PLUGIN_DATA`. For
compatibility with existing plugin packages, the same values are available as
`CLAUDE_PLUGIN_ROOT` / `CLAUDE_PLUGIN_DATA` and
`ECHO_PLUGIN_ROOT` / `ECHO_PLUGIN_DATA`. Paths are passed through the process
environment rather than interpolated into shell source, so spaces and shell
characters in an installation path remain valid.

Exit code `2` blocks the operation and uses stderr as the user-facing reason.
Other non-zero exits remain non-blocking but are surfaced in HookResult messages
instead of disappearing into logs.

For portable plugin reuse, embedding application also accepts Codex-style `systemMessage` and
`hookSpecificOutput` fields: `additionalContext`, `permissionDecision`,
`permissionDecisionReason`, `updatedInput`, and the PermissionRequest
`decision.behavior` object. Model-visible text is UTF-8-safely bounded before
it is merged into context.

These are the canonical wire names; `modified_input`, `message`, and
`permission_mode` are not aliases. `permission_mode_override` may be returned
by `PreToolUse` or `PermissionRequest`. It applies only to that tool call and is
passed into the permission service without mutating the session's configured
mode. Canonical values are `default`, `plan`, `auto-edit`, `full-auto`, `auto`,
`bubble`, `dont-ask`, and `strict`; the framework also accepts documented legacy
aliases when parsing.

Permission actions are reduced across the complete matching source set. The deterministic
source order (`UserConfig`, then `Plugin`, then `Skill`) affects diagnostics and equal-level
metadata, but not permission safety: `deny > ask > require_approval > allow`. An early
`allow` or `ask` therefore cannot short-circuit a later `deny`. A
`continue: false` attached to a permission-bearing command, HTTP, or programmatic
result is ignored for permission reduction; non-permission results retain normal
stop-propagation semantics.

### Sources, Reloading, and Dry Run

User, Skill, and Plugin hooks all use the same registration-time action
validation. Invalid actions are logged and omitted while valid actions in the
same rule remain active.

embedding application merges inline hooks from `application configuration`, global
`<application-data>/hooks.yaml`, and project `<application-data>/hooks.yaml`. Its watcher monitors all
three targets. Create, modify, atomic-replace, and remove events all trigger a
reload, so deleting a `hooks.yaml` removes its registered hooks without a
restart. A failed parse keeps the last known good registry. CLI, TUI, and GUI hook tests call
`HookRegistry::dry_run`: they evaluate event and matcher routing and report the
source/action list without executing side effects.

### Runtime Limits and Local Extension Policy

| Limit | Value | Purpose |
|-------|-------|---------|
| Default timeout | 600 seconds | Supports real command, MCP, HTTP, and Subagent work |
| Max timeout | 3600 seconds | Bounds accidentally unending hooks |
| Max command length | 32K characters | Rejects obviously malformed YAML |
| Sandbox execution | Optional | Hooks can run inside sandbox |

embedding application is a local, user-controlled application. Hook HTTP actions therefore allow
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

Skill files do not carry Hook configuration. Hooks are loaded from application
configuration or plugin Hook components.
