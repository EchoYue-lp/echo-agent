---
schema_version: 1
id: evidence.task-patch-claim-cas-verification
kind: evidence
observed_at: 78b9f06b4320531fd8f41260887cd69c1343e995
source_refs:
  - echo-orchestration/src/tasks/revisioned.rs
  - echo-sdk-protocol/tests/facade_inventory.rs
  - scripts/check-sdk-contracts.sh
  - scripts/check-language-sdks.sh
  - contracts/sdk/parity-manifest.json
  - contracts/sdk/facade-operation-catalog.json
  - contracts/sdk/source-contract.json
  - sdks/shared/contract-digests.json
  - sdks/typescript/test/catalog.test.js
  - sdks/python/tests/test_catalog.py
  - sdks/java/src/test/java/com/echoagent/sdk/FacadeParityTest.java
supports: [behavior.task-subagent-execution, rule.task-subagent-authority, behavior.sdk-facade-routing, rule.sdk-rust-authority]
limitations:
  - 本地证据不替代远端 Linux、Windows 或发布环境 CI
  - 未对第三方 RevisionedTaskStore 实现执行跨进程并发验收
---

# Task execution CAS 与 SDK 合同验证证据

## 支持的结论

旧实现上的确定性 red 测试以 `load Pending -> claim -> stale Skip commit` 重现 live claim 被覆盖；当前实现的 revisioned Task 测试覆盖 canonical service producer、claim 与 Skip、SetStatus、spec Update、settlement、retry 的交错，并保留正常 patch、manual progress 与 claim settlement 行为。定向 Clippy 和根 crate `subagent` feature check 均通过。

SDK 唯一生成链把 `TaskGraphCommit::expected_executions` 分类为 `value:task` 的 `external_contract`。canonical identity 总量为 9683，其中 external contract 5607、Host/Rust-only 1765、language intrinsic 780、internal helper 90、deferred 1441；extension protocol 仍为 1。`check-sdk-contracts.sh` 验证 90 个 artifact、Rust inventory 与 5/29/75 组合同测试；`check-language-sdks.sh` 验证 TypeScript 156、Python 168 和 Java Host quickstart，全部退出码为 0。

## 来源与范围

验证覆盖生成物可重复、operation catalog/shared digest 一致、Rust facade inventory、三语言 scope consumer 与真实 Host 连接。它证明本次 public precondition 已进入既有 SDK 漂移合同，不把 9683 个 identity 重新解释为项目治理完成度。

## 已知缺口

外部 Store 的原子 compare 仍由其实现负责；本切片只证明 framework 内存实现、public contract 和当前语言消费者闭合。
