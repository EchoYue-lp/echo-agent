---
schema_version: 1
id: evidence.k8s-sandbox-cleanup-settlement-verification
kind: evidence
observed_at: source:ebb9b2db3cc55a1e63eeea959d10359c8da2d72c9e0e6e2ea27026186bc87c48
source_refs:
  - echo-execution/src/sandbox/k8s.rs
  - docs/adr/0002-sandbox-cancellation-cleanup.md
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 未连接真实Kubernetes集群，finalizer与不可达node行为依据官方合同和fake-kubectl故障注入
  - 按并发磁盘约束只运行echo_execution focused门禁，未运行全workspace门禁或远端CI
---

# K8s Sandbox cleanup settlement验证证据

## 支持的结论

修复前回归`caller_abort_after_pod_submission_keeps_cleanup_owner`以exit 101失败：fake-kubectl
观察到`run`后，caller abort在1.66秒内未产生`delete`。修复后K8s定向测试16项全部通过，
证明成功、非零退出、timeout、cancel和caller drop均在terminal前到达delete；delete spawn、
非零退出和timeout均成为可见typed cleanup debt，且success/nonzero/timeout/cancel facts被保留。
新增反例还证明kubectl leader退出后遗留的pipe holder被进程组结算、stdin失败与blocked stdin
caller drop进入同一cleanup、delete完成/失败可被观察，以及JoinError补偿cleanup在waiter drop后继续。
独立review首轮发现并阻断无界pipe drain、cleanup debt交接窗口和stdin/settlement测试缺口；修复后
第二轮复审PASS，Critical、Important、Minor均为0。

## 来源与范围

以下命令均在源码摘要`ebb9b2db3cc55a1e63eeea959d10359c8da2d72c9e0e6e2ea27026186bc87c48`
上以`CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
CARGO_BUILD_JOBS=2`执行并返回0：

- `cargo test -p echo_execution sandbox::k8s::tests --locked -- --nocapture`：16 passed，0 failed；
- `cargo check -p echo_execution --all-features --locked`；
- `cargo clippy -p echo_execution --all-targets --all-features --locked -- -D warnings`；
- `cargo clippy -p echo_execution --lib --all-features --locked -- -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable`；
- `cargo fmt -p echo_execution`与`cargo fmt -p echo_execution -- --check`。

## 已知缺口

fake-kubectl提供确定性的client/process/delete故障，不证明特定集群CNI、admission controller、
finalizer controller或失联node的运行时延迟；真实集群验收继续作为部署环境证据，而不是本地
单元门禁的替代品。
