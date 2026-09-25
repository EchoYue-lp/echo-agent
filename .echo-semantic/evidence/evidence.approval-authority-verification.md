---
schema_version: 1
id: evidence.approval-authority-verification
kind: evidence
observed_at: source:e2f3b5f8a9af67e4a9534a49815f6da72fb3c17c5834b86ce5221c121838a99f
source_refs:
  - echo-core/src/tools/permission.rs
  - echo-orchestration/src/human_loop/service.rs
  - echo-tools/src/shell.rs
  - src/agent/react/run/pipeline.rs
  - docs/adr/0075-invocation-approval-receipt.md
supports: [finding.approval-authority]
limitations:
  - focused checks do not replace the required PR merge gate or remote CI
  - no real EKO UI provider or cross-process receipt transport is exercised
command_results:
  - { command: "cargo test -p echo_core approval_receipt_matches_only_exact_canonical_effective_input --locked", exit_code: 0 }
  - { command: "cargo test -p echo_orchestration modified_input_is_returned_with_its_permission_decision --locked", exit_code: 0 }
  - { command: "cargo test -p echo_tools --features shell approval_receipt_is_required_and_bound_for_foreground_shell --locked", exit_code: 0 }
  - { command: "cargo test -p echo_tools --features shell background_and_streaming_paths_consume_the_same_exact_receipt --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent --features human-loop,shell permission_service_receipt_reaches_real_shell_effect_boundary --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent --features human-loop,shell permission_request_hook_allow_rewrite_binds_receipt_to_final_shell_args --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent --features human-loop,shell permission_hooks_trace_real_allow_and_deny_on_the_main_path --locked", exit_code: 0 }
  - { command: "cargo clippy -p echo_core -p echo_orchestration -p echo_execution -p echo_tools -p echo_agent --features human-loop,shell --all-targets --locked -- -D warnings", exit_code: 0 }
  - { command: "cargo clippy -p echo_agent --features human-loop,shell --lib --bins --locked -- -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable", exit_code: 0 }
  - { command: "cargo check -p echo_agent --features human-loop,shell --locked", exit_code: 0 }
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
  - { command: "git diff --check", exit_code: 0 }
---

# Issue 37 Permission receipt focused verification

## 支持的结论

## Result

The canonical receipt test, rewritten-input PermissionService test, and
foreground/background/streaming Shell tests passed.  The Agent crate compiled
with human-loop and shell enabled; formatter and diff checks passed.

## 来源与范围

These commands were run in the dedicated repair worktree against the current
framework candidate. They do not prove the remote PR, full merge gate, or
post-merge semantic continuity.

## 已知缺口

The full workspace gate, feature matrix, independent rereview, remote CI, and
main-branch delivery remain pending.
