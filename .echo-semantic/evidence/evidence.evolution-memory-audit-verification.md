---
schema_version: 1
id: evidence.evolution-memory-audit-verification
kind: evidence
observed_at: source:0a91548f9c8d3f6e6a19bc2025fd2d21a46b656162f50b44aedfba5b6d5bf1bb
source_refs:
  - src/evolution/layer.rs
  - src/evolution/mutation.rs
  - src/evolution/audit.rs
  - src/evolution/review.rs
  - src/evolution/runtime_integration.rs
  - src/tools/builtin/memory.rs
  - src/memory_promoter.rs
  - echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs
supports: [finding.evolution-audit-atomicity, behavior.eval-evolution]
limitations:
  - Combined gates, semantic snapshot and integrated review receipts are owned by evidence.foundation-36-72-51-integration-verification
  - Remote-main delivery remains outside this focused evidence
  - Fault injection simulates crash windows and settled Store rollback, not a physical power cut
---

# Issue 51 focused verification

## 支持的结论

首个失败测试 `durable_write_reconciles_one_audit_after_restart` 在旧实现因缺恢复入口
退出 101；修复后对应恢复回归通过。当前候选的 `cargo test -p echo_agent evolution:: --lib
--locked` 退出 0，164 tests；`cargo test -p echo_agent memory --lib --locked` 退出 0，
27 tests；`cargo test -p echo-agent-learning --test example_contracts
contract_demo51_layered_memory_recovery_api --features eval,improve --locked` 退出 0，
1 test；`cargo clippy -p echo_agent --lib --locked -- -D warnings`、
`cargo clippy -p echo_agent --lib --bins --locked -- -D clippy::unwrap_used
-D clippy::expect_used -D clippy::panic -D clippy::unreachable`、
`cargo check -p echo_agent --lib --no-default-features --locked` 与
`cargo fmt --all -- --check` 均退出 0。
测试覆盖 prepare 零写入、audit 失败后重启、hot/warm 半途迁移、warm 删除/meta、
多 manager 同根写入、merge 第二条 audit 失败和成员中途投影、settled Store 回退、
未知外部 Store 变更失败关闭、业务 audit ID 去重及 manager 读围栏。
新增首尾空白/多行内容的旧hot投影冲突红测，以及两manager交错的旧值覆盖新写入
红测；修复后覆盖hot无损往返、重启、降级、后续写入，以及warm/hot降级和晋升的
prepare前旧值复核。

## 来源与范围

上述命令在独立 `fix/Echoyue/issue-51-evolution-audit` 工作树执行，使用
`CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
CARGO_BUILD_JOBS=2` 节省本机磁盘；结果来自各命令完整退出状态。

## 集成收据

本 Evidence 只保留 #51 focused 命令与故障注入结论。合并源码上的 workspace、
feature、semantic 与 integrated review 精确收据统一归
`evidence.foundation-36-72-51-integration-verification`，不在三份 lane Evidence
中重复维护。

## 已知缺口

本地集成收据不证明 remote-main delivery，也不等价于物理断电故障注入。
