---
schema_version: 1
id: evidence.plan-mode-write-surface-repair
kind: evidence
observed_at: source:0427aee5ee15ea51f4623b1ae3db84522ef774c616f10390c3d7da16064d2ec0
source_refs:
  - echo-core/src/tools/mod.rs
  - echo-tools/src/files/files.rs
  - echo-tools/src/git.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/snapshot.rs
  - docs/adr/0071-protected-path-and-readonly-tool-boundary.md
supports: [finding.plan-mode-write-surface, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - Custom Tool authors must declare their capabilities truthfully; the framework cannot infer arbitrary external effects
  - Direct programmatic ToolManager calls are outside the Agent Plan-mode policy path
  - Full workspace gates, remote CI, independent rereview, and Issue closure remain pending
---

# Issue 70 Plan-mode write-surface repair

## 支持的结论

`ToolRuntime::tools_for_llm` hides tools whose `ToolCapabilities::is_read_only()` is false in
Plan mode. `PlanModeStage` uses the same capability fact at invocation and blocks mutation before
PreToolUse Hook, Permission, and Execute. `ExecuteStage` now repeats the live check immediately
before the first tool side effect, including the readonly Agent hard lower bound, so a mode change while a Hook or approval stage is awaiting
cannot be bypassed by a stale call-scoped Allow. A late denial is normalized to the existing
blocked/Unavailable policy result while trace, audit, and terminal callback stages still observe
the failed ToolResult. Plan and readonly-Agent denials retain separate reasons and permission
sources. File append mutation flushes before returning its confirmed `FileEdit` effect, so a
subsequent read cannot observe a stale pre-effect state. Git branch and commit declare dangerous risk,
so they are excluded without a name blacklist. `ToolExecutionPipeline` has a private stage list
and exposes only its default constructor through the public safe API.

## 来源与范围

The capability-based Plan gate entered main in `3735f7e0`. `f7c1fef7` added the read-only Agent
surface using the same capability fact and retained the Plan gate's ordering. This repair adds
the effect-boundary recheck on top of that authority and checks every physical non-streaming or
streaming attempt after validation, permit acquisition, and retry delay. ADR 0071 records the
shared policy boundary. This covers framework Agent automatic tool invocation, including mutating
MCP tools; tool authors own the correctness of their declared capabilities.

## 已知缺口

The durable regression and focused verification are recorded in
`evidence.plan-mode-write-surface-timing-verification`. Finding #70 remains open until the
independent rereview, PR/CI, remote main, and Issue closure are complete.
