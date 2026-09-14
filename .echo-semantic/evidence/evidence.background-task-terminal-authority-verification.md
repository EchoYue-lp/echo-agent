---
schema_version: 1
id: evidence.background-task-terminal-authority-verification
kind: evidence
observed_at: 9d1f3f2b5fdc204c08ecdec32ed22e8df95870e9
source_refs:
  - echo-orchestration/src/tasks/background_task.rs
  - docs/en/29-long-running-tasks.md
  - docs/zh/29-long-running-tasks.md
  - docs/adr/0039-background-task-terminal-authority.md
  - contracts/sdk/parity-manifest.json
  - contracts/sdk/source-contract.json
  - sdks/shared/contract-digests.json
  - echo-sdk-protocol/tests/facade_inventory.rs
  - scripts/check-sdk-contracts.sh
  - scripts/check-language-sdks.sh
  - sdks/typescript/test/catalog.test.js
  - sdks/python/tests/test_catalog.py
  - sdks/java/src/test/java/com/echoagent/sdk/FacadeParityTest.java
supports: [behavior.task-subagent-execution, rule.task-subagent-authority, behavior.sdk-facade-routing, rule.sdk-rust-authority]
limitations:
  - 完整workspace合并门禁与远端CI尚未执行
  - 项目约束禁止新增panic-producing测试，panic归约由JoinError代码路径、Tokio合同和独立review验证
---

# BackgroundTask terminal authority 验证证据

## 支持的结论

三个旧实现red均命中1项并exit 101：multi-waiter报告terminal waiter仍blocked；queued cancel未settled；zero concurrency保持Pending。修复后的BackgroundTask模块18 tests全部通过，覆盖NonClone result的真实Clone handles、单消费者result/多观察者status、retry wait、queued cancel/deadline、zero config、execution cancel/timeout child DropProbe、准确type-erased Failed/Cancelled和retention，以及普通错误复制panic文案前缀仍不会被误分类的反例。

整个`echo_orchestration`通过336 unit tests及11 passed/5 ignored doc tests；root facade smoke 10和documentation contract 5通过。Orchestration all-target Clippy、panic-policy Clippy与crate check通过。

SDK生成差异只有BackgroundTask Clone的1个canonical和3个re-export alias，全部language_intrinsic；canonical计数9683->9684，language intrinsic 780->781，external 5607、Host/Rust-only 1765、helper 90、deferred 1441不变。90 artifacts在完整check中匹配；facade inventory 75、TS156、Python168与Java Host quickstart通过，三语言没有新增BackgroundTask facade。

## 来源与范围

red/green、工程和SDK日志位于`.supreme/logs/plan17-*`。生成链为`export_schema --update`、shared catalog export、facade frozen snapshot和language SDK gate；计数与intrinsic membership均经过人工diff复核。

## 已知缺口

完整SDK check首次运行在旧scope计数780处失败；计数与intrinsic摘要修正后，review后的完整SDK check、facade inventory与全部语言gate均通过。未执行真实runtime shutdown、跨平台stress或长时间高并发等待。
