---
schema_version: 1
id: evidence.sandbox-minimum-isolation-verification
kind: evidence
observed_at: bd17c73075d6b3cf8e00877fa0fb10d36694ea54
source_refs:
  - echo-core/src/sandbox.rs
  - echo-execution/src/sandbox/policy.rs
  - echo-execution/src/sandbox/manager.rs
supports: [finding.sandbox-minimum-isolation, behavior.effect-permission-execution]
limitations:
  - The isolated full local gate passed on e8371e58; PR/CI and remote main remain unverified
  - No real Docker or Kubernetes outage was induced; tests use unavailable configured backends
command_results:
  - { command: "cargo test -p echo_execution sandbox::manager::tests --locked", exit_code: 0 }
  - { command: "CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/issue-61-83-70-38-evidence/target CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 ./scripts/verify.sh", exit_code: 0 }
---

# Issue 83 explicit minimum verification

## 支持的结论

在 `origin/main@f7c1fef7` 集成源码上，`sandbox::manager::tests` 12 passed、0 failed。
`explicit_minimum_never_falls_back_to_process` 覆盖 fallback 开启时 buffered、limited、
stream 三入口的拒绝；`unavailable_configured_container_does_not_claim_available_os_isolation`
覆盖配置了不可用 Docker 时的实际隔离判定。无显式最低级别的策略偏好仍可按配置 fallback。
证据分支 `e8371e58` 的 `./scripts/verify.sh` 使用独立 target 退出 0，覆盖 fmt check、
两档 workspace Clippy、workspace all-target/all-feature 测试与 no-default-features
library check。

## 来源与范围

执行时设置共享 parent `CARGO_TARGET_DIR`、`CARGO_BUILD_JOBS=2`、`CARGO_INCREMENTAL=0`、
`CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_PROFILE_TEST_DEBUG=0`。代码与测试位于
`echo-execution/src/sandbox/manager.rs`，caller floor 定义位于 `echo-core/src/sandbox.rs`。
完整门禁由主任务在本隔离工作树运行并回报，target 未纳入 Git。

## 已知缺口

独立复审另见 `audit.sandbox-minimum-isolation-rereview`。真实 Docker/Kubernetes 故障、
远端 CI/main 与 Issue #83 关闭尚无收据。
