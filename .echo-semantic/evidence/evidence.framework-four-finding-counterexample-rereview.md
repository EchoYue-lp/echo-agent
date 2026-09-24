---
schema_version: 1
id: evidence.framework-four-finding-counterexample-rereview
kind: evidence
observed_at: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
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
  - The e8371e58 branch passed the isolated full local gate; PR/CI and remote main remain pending
  - Independent rereview supports the #61/#83 original counterexamples only; #70/#38 remain open
  - No remote PR, CI, merge, or Issue closure evidence
  - Background Review still has no outcome receipt when its caller drops a future after a partial persistence write
  - Issue 70 has a confirmed live Plan-switch execution counterexample and remains open
---

# Framework Finding counterexamples at origin/main@f7c1fef7

## 支持的结论

- **#61:** `InMemoryAuditLogger::log` recovers a poisoned write lock before pushing the event; query, snapshot, length, and clear also recover. The injected poison test confirms the second event is stored and visible, so the original silent successful drop is not reproduced.
- **#83:** `SandboxPolicy` preserves an explicit `minimum_isolation`; `SandboxManager` rejects a selected fallback below it in buffered, limits, and stream execution. Tests cover an available process backend, an unavailable configured container, and fallback enabled.
- **#70:** Model-visible tools and `PlanModeStage` use the same `ToolCapabilities::is_read_only` classification. Git branch/commit declare dangerous risk; a mutating MCP call is hidden and blocked when Plan is active before the gate runs. A deterministic late mode switch after that gate permits one mutating execution through Hook Allow. The original name-list bypass is gone, but the read-only contract remains open; see `evidence.plan-mode-write-surface-timing-verification`.
- **#38:** The old detached `JoinHandle` API is gone: `review`, `review_and_wait`, and `review_by_run_id` return awaited outcomes, and dropping an unpolled future starts no review. Panic and partial write errors are visible when awaited. The existing cancellation test also proves the remaining gap: after memory has been written but before the observer finishes, dropping the review future leaves the write while the caller receives no outcome. There is no review deadline or durable receipt/owner for that path; keep the Finding open.

## 来源与范围

The first-round commands ran at `53628a3e` with `CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/target`, `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, and `CARGO_PROFILE_TEST_DEBUG=0`:

| Command | Result |
| --- | --- |
| `cargo test -p echo_state poisoned_write_lock_recovers_log_query_snapshot_and_clear --locked` | exit 0; 1 passed, 0 failed |
| `cargo test -p echo_execution sandbox::manager::tests --locked` | exit 0; 12 passed, 0 failed |
| `cargo test -p echo_agent --features mcp,human-loop permission_plan --locked` | exit 0; 2 passed, 0 failed |
| `cargo test -p echo_agent --features mcp,human-loop evolution::background_review::tests --locked` | exit 0; 15 passed, 0 failed |

After merging `origin/main@f7c1fef7`, the same #61 focused test passed 1/1, the #83 manager
suite passed 12/12, and the two existing #70 `permission_plan` tests passed 2/2. The additional
deterministic #70 late Plan-switch probe failed 0/1 with one mutating execution; its exact command
and interleaving are in `evidence.plan-mode-write-surface-timing-verification`. The #38 source path
is unchanged and still permits drop after a partial memory write without an outcome receipt; its
15-test suite above was not separately rerun on this integrated snapshot. The clean `e8371e58`
branch then passed `./scripts/verify.sh` using this worktree's isolated target, including its
workspace all-target/all-feature tests. A prior shared-target attempt failed against a stale
`echo_orchestration` API cache; the isolated rerun is the final local gate. Strict semantic
snapshot checking passed after removal of the temporary failing probe.

## 已知缺口

#61 and #83 now have separate independent rereview audit receipts and are resolved in this
evidence branch. #70 and #38 remain open for the concrete gaps above; the clean branch's full
local gate does not resolve the temporary #70 red probe. PR/CI and Issue closure remain pending.
