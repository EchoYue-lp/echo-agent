---
schema_version: 1
id: evidence.framework-four-finding-counterexample-rereview
kind: evidence
observed_at: 53628a3ebf7fab6fc0c8910d83147e90d8de4dc0
source_refs:
  - echo-state/src/audit/memory.rs
  - echo-core/src/sandbox.rs
  - echo-execution/src/sandbox/policy.rs
  - echo-execution/src/sandbox/manager.rs
  - echo-core/src/tools/mod.rs
  - echo-tools/src/git.rs
  - src/agent/snapshot.rs
  - src/agent/react/run/pipeline.rs
  - src/evolution/background_review.rs
supports: [finding.in-memory-audit-successful-drop, finding.sandbox-minimum-isolation, finding.plan-mode-write-surface, finding.background-review-detached-persistence-settlement]
limitations:
  - Focused tests and source inspection only; no full workspace gate or integrated source-digest verification
  - This evidence artifact has not received an independent final rereview
  - No remote PR, CI, merge, or Issue closure evidence
  - Background Review still has no outcome receipt when its caller drops a future after a partial persistence write
---

# Framework Finding counterexamples at origin/main@53628a3e

## 支持的结论

- **#61:** `InMemoryAuditLogger::log` recovers a poisoned write lock before pushing the event; query, snapshot, length, and clear also recover. The injected poison test confirms the second event is stored and visible, so the original silent successful drop is not reproduced.
- **#83:** `SandboxPolicy` preserves an explicit `minimum_isolation`; `SandboxManager` rejects a selected fallback below it in buffered, limits, and stream execution. Tests cover an available process backend, an unavailable configured container, and fallback enabled.
- **#70:** Model-visible tools and `PlanModeStage` use the same `ToolCapabilities::is_read_only` classification. The stage runs before Hook, Permission, and Execute; git branch/commit declare dangerous risk. Mutating MCP calls are hidden and blocked both for configured Plan mode and a live mode change despite Hook Allow. `ToolExecutionPipeline` exposes only the default stage constructor through its public safe API, so the builder's pipeline injection does not expose a stage-removal route.
- **#38:** The old detached `JoinHandle` API is gone: `review`, `review_and_wait`, and `review_by_run_id` return awaited outcomes, and dropping an unpolled future starts no review. Panic and partial write errors are visible when awaited. The existing cancellation test also proves the remaining gap: after memory has been written but before the observer finishes, dropping the review future leaves the write while the caller receives no outcome. There is no review deadline or durable receipt/owner for that path; keep the Finding open.

## 来源与范围

All commands ran in this isolated worktree at `53628a3e` with `CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/target`, `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, and `CARGO_PROFILE_TEST_DEBUG=0`:

| Command | Result |
| --- | --- |
| `cargo test -p echo_state poisoned_write_lock_recovers_log_query_snapshot_and_clear --locked` | exit 0; 1 passed, 0 failed |
| `cargo test -p echo_execution sandbox::manager::tests --locked` | exit 0; 12 passed, 0 failed |
| `cargo test -p echo_agent --features mcp,human-loop permission_plan --locked` | exit 0; 2 passed, 0 failed |
| `cargo test -p echo_agent --features mcp,human-loop evolution::background_review::tests --locked` | exit 0; 15 passed, 0 failed |

These results support a lane-local rereview of the original counterexamples.

## 已知缺口

Finding status, integrated semantic evidence references, full merge gates, and remote delivery remain pending. In particular, #38 still lacks an outcome receipt when cancellation follows a partial memory write.
