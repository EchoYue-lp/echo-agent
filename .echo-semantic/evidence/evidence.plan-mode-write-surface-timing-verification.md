---
schema_version: 1
id: evidence.plan-mode-write-surface-timing-verification
kind: evidence
observed_at: source:c4ea5a4467fdfe6b2559835c2e6b02e20a5456fbe941b86fbeb7753c2eb16fa4
source_refs:
  - src/agent/react/run/pipeline.rs
  - src/agent/snapshot.rs
  - echo-tools/src/files/files.rs
  - echo-orchestration/src/human_loop/service.rs
  - echo-core/src/hooks/types.rs
supports: [finding.plan-mode-write-surface, behavior.effect-permission-execution]
limitations:
  - The ToolManager regression proves admission runs after a held concurrency permit; the retry loop uses the same admission callback but has no separate delay-specific race fixture
  - Third-party Tool capability declarations remain trusted input
  - Full workspace gates, remote CI, independent rereview, and Issue closure remain pending
command_results:
  - { command: "cargo test -p echo_agent --features mcp,human-loop live_permission_plan_precedes_hook_allow_for_mutating_mcp_tool --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent --features mcp,human-loop live_plan_mode_is_rechecked_after_call_scoped_allow --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent --features mcp readonly_surface_is_rechecked_after_tool_replacement --locked", exit_code: 0 }
  - { command: "cargo test -p echo_execution --lib checked_admission_runs_after_permit_and_can_reject_before_effect --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent --features mcp,human-loop readonly_tools_hides_and_blocks_custom_mutation --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent --features mcp,human-loop tool_visibility_combines_skill_plan_and_disabled_policies --locked", exit_code: 0 }
  - { command: "cargo clippy -p echo_agent --lib --features mcp,human-loop --locked -- -D warnings", exit_code: 0 }
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
---

# Issue 70 live Plan switch repair verification

## 支持的结论

The pre-repair timing counterexample at `origin/main@f7c1fef7` showed one mutating execution
after a live Plan switch. The durable regression
`live_plan_mode_is_rechecked_after_call_scoped_allow` now drives the real caller through a
programmatic `PreToolUse` Hook that pauses, switches `PermissionService` to Plan, then returns
`Allow`; the full pipeline finishes with a typed `Unavailable` denial and execution count 0.

The probe registered a mutating MCP Tool and a programmatic PreToolUse Hook. The Hook sent an
entry signal, then waited on a `Notify`. After receiving the signal, the test awaited
`PermissionService::set_mode(Plan)`, released the Hook, and returned `HookResult::allow()`.
The core interleaving was:

```rust
// Inside the programmatic PreToolUse Hook:
entered.notify_one();
release.notified().await;
HookResult::allow()

// Concurrent control future after the entry signal:
service.set_mode(PermissionMode::Plan).await;
release.notify_one();
```

The repair makes `ExecuteStage` recheck `ctx.plan_mode`, any call-scoped Plan override, and the
live `PermissionService` mode before the first side effect. A stale Allow is recorded as a Plan
denial, normalized to the existing blocked/Unavailable contract, and the mutating Tool is never
invoked. `checked_admission_runs_after_permit_and_can_reject_before_effect` additionally holds
the only ToolManager permit until the caller releases it, then proves the admission callback runs
before the target Tool and can reject without an effect.

## 来源与范围

The durable test uses the same mutating MCP probe and shared PermissionService as the existing
Plan test, then calls the real `execute_tool_with_policy` caller. It records the call-scoped Allow,
switches the live service to Plan while the Hook is suspended, releases the Hook, and asserts the
final execution gate blocks before mutation. The ToolManager test covers the post-permit boundary
independently of the Agent policy pipeline. The readonly replacement regression runs
`PlanModeStage`, replaces the current tool generation, and confirms the final admission still
blocks a mutating implementation under `readonly_tools`.

## 已知缺口

The repair is focused to the framework execution boundary. PR/CI, remote main, independent
rereview, and Issue acceptance remain pending.
