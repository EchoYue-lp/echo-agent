---
schema_version: 1
id: evidence.sdk-gap-generation-validation-verification
kind: evidence
observed_at: 01c203b85a3451cf0c3bcfc91e68ad25534f8041
source_refs:
  - sdks/python/tests/test_lifecycle.py
  - sdks/java/src/test/java/com/echoagent/sdk/LifecycleTest.java
  - sdks/typescript/src/client.ts
supports: [behavior.protocol-projection, behavior.sdk-facade-routing, rule.protocol-role-separation, rule.sdk-rust-authority]
limitations:
  - 完整workspace门禁留到汇总分支发起MR前执行
  - Host gap ACK replay由独立Finding覆盖
---

# SDK gap generation validation验证证据

## 支持的结论

Python focused生命周期测试36项通过，全套175项通过、1项skip；Java Lifecycle与全套Maven
测试均退出0。Ruff import/style与format检查、Git diff check通过。反例覆盖同ID错generation、
错kind、倒退/不连续sequence、非法gap、重复gap、ACK失败以及notification-before-subscription
的错代buffer，均证明错误不会推进cursor或ACK。

## 来源与范围

TypeScript完整handle检查作为已存在对照；Python与Java测试直接驱动各自真实incoming
notification和publisher路径，不以catalog计数替代行为验证。

## 已知缺口

本证据不声称Host gap->ACK->live replay已闭合；该跨端场景在Issue #120继续验证。
