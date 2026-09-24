---
schema_version: 1
id: evidence.sandbox-minimum-isolation-verification
kind: evidence
observed_at: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
source_refs:
  - echo-core/src/sandbox.rs
  - echo-execution/src/sandbox/policy.rs
  - echo-execution/src/sandbox/manager.rs
supports: [finding.sandbox-minimum-isolation, behavior.effect-permission-execution]
limitations:
  - Focused manager tests only; no current-branch full workspace gate or remote delivery
  - No real Docker or Kubernetes outage was induced; tests use unavailable configured backends
command_results:
  - { command: "cargo test -p echo_execution sandbox::manager::tests --locked", exit_code: 0 }
---

# Issue 83 explicit minimum verification

## 支持的结论

在 `origin/main@f7c1fef7` 集成源码上，`sandbox::manager::tests` 12 passed、0 failed。
`explicit_minimum_never_falls_back_to_process` 覆盖 fallback 开启时 buffered、limited、
stream 三入口的拒绝；`unavailable_configured_container_does_not_claim_available_os_isolation`
覆盖配置了不可用 Docker 时的实际隔离判定。无显式最低级别的策略偏好仍可按配置 fallback。

## 来源与范围

执行时设置共享 parent `CARGO_TARGET_DIR`、`CARGO_BUILD_JOBS=2`、`CARGO_INCREMENTAL=0`、
`CARGO_PROFILE_DEV_DEBUG=0`、`CARGO_PROFILE_TEST_DEBUG=0`。代码与测试位于
`echo-execution/src/sandbox/manager.rs`，caller floor 定义位于 `echo-core/src/sandbox.rs`。

## 已知缺口

当前证据只证明 focused 故障反例；独立复审另见
`audit.sandbox-minimum-isolation-rereview`。完整 workspace 门禁、远端 CI 与 Issue 关闭
尚无收据。
