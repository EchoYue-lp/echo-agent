---
schema_version: 1
id: evidence.plan-mode-write-surface-timing-verification
kind: evidence
observed_at: f7c1fef779fd83d2f26a780edc5edf1babbc4c5d
source_refs:
  - src/agent/react/run/pipeline.rs
  - src/agent/snapshot.rs
  - echo-orchestration/src/human_loop/service.rs
  - echo-core/src/hooks/types.rs
supports: [finding.plan-mode-write-surface, behavior.effect-permission-execution]
limitations:
  - The deterministic failing probe was a temporary uncommitted test and is not part of the docs-only deliverable
  - Existing Plan tests cover mode active before invocation; they do not close the late-switch interleaving
  - No production repair, full workspace gate, or remote delivery is claimed
command_results:
  - { command: "cargo test -p echo_agent --features mcp,human-loop permission_plan --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent --features mcp,human-loop plan_switch_during_pre_tool_hook_blocks_mutating_mcp_tool --locked", exit_code: 101 }
---

# Issue 70 live Plan switch counterexample

## 支持的结论

At `origin/main@f7c1fef7`, the two existing `permission_plan` tests passed (2/2). A temporary
deterministic test named `plan_switch_during_pre_tool_hook_blocks_mutating_mcp_tool`, inserted
after `live_permission_plan_precedes_hook_allow_for_mutating_mcp_tool` in `src/agent/snapshot.rs`,
failed twice; the final run exited 101 with 0 passed, 1 failed and the assertion at line 3847:
`mutating tool executed after a live Plan switch`, actual execution count 1 versus expected 0.

The probe registered a mutating MCP Tool and a programmatic PreToolUse Hook. The Hook sent an
entry signal, then waited on a `Notify`. After receiving the signal, the test awaited
`PermissionService::set_mode(Plan)`, released the Hook, and returned `HookResult::allow()`.
The core interleaving was:

```rust
// Inside the programmatic PreToolUse Hook:
let _ = entered_tx.send(());
release.notified().await;
HookResult::allow()

// Concurrent control future after the entry signal:
service.set_mode(PermissionMode::Plan).await;
release.notify_one();
```

`PlanModeStage` had already observed Default mode. `PreToolUseHookStage` then returned Allow;
`PermissionStage` consumed that Allow without rechecking Plan, and `ExecuteStage` invoked the
mutating Tool. This is a remaining read-only surface violation, so Finding #70 stays open.

## 来源与范围

The temporary test used the same shared target and build environment as the #61/#83 focused
runs. The final document records its exact name, synchronization, command, failure count, and
execution counter so a repair branch can add a durable red/green regression. The exploratory
test is removed from this evidence-only branch to keep its test suite runnable.

## 已知缺口

The effect-boundary repair and its regression test belong to a separate implementation lane.
An independent reviewer, full gate, and remote Issue acceptance remain pending.
