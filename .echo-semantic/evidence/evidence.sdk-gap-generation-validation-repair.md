---
schema_version: 1
id: evidence.sdk-gap-generation-validation-repair
kind: evidence
observed_at: 01c203b85a3451cf0c3bcfc91e68ad25534f8041
source_refs:
  - sdks/python/src/echo_agent_sdk/client.py
  - sdks/java/src/main/java/com/echoagent/sdk/BoundedPublisher.java
  - sdks/java/src/main/java/com/echoagent/sdk/EchoAgentClient.java
supports: [behavior.protocol-projection, behavior.sdk-facade-routing, rule.protocol-role-separation, rule.sdk-rust-authority]
limitations:
  - Host gap ACK后的live replay watermark由finding.sdk-gap-ack-replay-watermark独立追踪
  - 本修复不改变wire schema、Host event ledger或TypeScript既有实现
---

# SDK gap generation validation修复证据

## 支持的结论

Python与Java incoming event/gap现在都以完整stream handle的id、generation与kind绑定feed，
并检查同代sequence连续性、gap range、from cursor与snapshot watermark。验证失败发生在
consumer delivery和ACK之前，不推进cursor；重复gap保持幂等。

Python预订阅queue会在bind时丢弃错代buffer并关闭feed，Java publisher在subscriber
成功接收后才推进ACK。两种语言与TypeScript已有完整handle语义对齐。

## 来源与范围

修复只修改source SDK client/publisher，不改变Rust Host、schema或extension method。
代码以commit `01c203b85a3451cf0c3bcfc91e68ad25534f8041`进入汇总分支。

## 已知缺口

Host gap ACK后从旧live watermark恢复可能重发已被snapshot覆盖的event；该问题已经拆为
独立Finding `finding.sdk-gap-ack-replay-watermark`与Issue #120，不构成放宽SDK校验的理由。
