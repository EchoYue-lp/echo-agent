---
schema_version: 1
id: evidence.plan-mode-write-surface-repair
kind: evidence
observed_at: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
source_refs:
  - echo-core/src/tools/mod.rs
  - echo-tools/src/git.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/snapshot.rs
  - docs/adr/0071-protected-path-and-readonly-tool-boundary.md
supports: [finding.plan-mode-write-surface, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - Custom Tool authors must declare their capabilities truthfully; the framework cannot infer arbitrary external effects
  - Direct programmatic ToolManager calls are outside the Agent Plan-mode policy path
  - A live switch to Plan after PlanModeStage has run is not rechecked before Execute
  - This source inspection does not establish current-branch tests, full gates, or remote delivery
---

# Issue 70 partial Plan-mode write-surface repair

## 支持的结论

`ToolRuntime::tools_for_llm` hides tools whose `ToolCapabilities::is_read_only()` is false in
Plan mode. `PlanModeStage` uses the same capability fact at invocation and blocks mutation before
PreToolUse Hook, Permission, and Execute. A live `PermissionService` mode change is checked at
invocation. Git branch and commit declare dangerous risk, so they are excluded without a name
blacklist. `ToolExecutionPipeline` has a private stage list and exposes only its default
constructor through the public safe API.

## 来源与范围

The capability-based Plan gate entered main in `3735f7e0`. `f7c1fef7` added the read-only Agent
surface using the same capability fact and retained the Plan gate's ordering. ADR 0071 records
the shared policy boundary. This covers Plan mode active when the gate runs for framework Agent
automatic tool invocation, including mutating MCP tools; tool authors own the correctness of
their declared capabilities.

## 已知缺口

The timed mode-switch counterexample is recorded separately in
`evidence.plan-mode-write-surface-timing-verification`. This repair is partial; Finding #70
remains open. Full gates, independent rereview, and remote Issue closure are pending.
