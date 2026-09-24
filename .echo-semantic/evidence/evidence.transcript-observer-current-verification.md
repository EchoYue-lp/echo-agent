---
schema_version: 1
id: evidence.transcript-observer-current-verification
kind: evidence
observed_at: 733d352fc719f922b21bab1cd46206139564367f
source_refs:
  - src/agent/react/run/phases/finalize.rs
  - src/agent/react/run/phases/compact.rs
  - src/agent/react/run/phases/tools.rs
  - src/agent/react/run/stream_channel.rs
supports: [finding.transcript-projection-settlement]
limitations:
  - Final-digest framework gates are recorded by the framework-only closure evidence; consumer acceptance is out of scope
---

# Issue 106 observer verification

## 支持的结论

`closed_settlement_observer_preserves_persisted_projection_fact` 是 closed channel 后不重复
save 的直接回归。最终源码摘要继续执行该 focused test 与 stream/direct terminal 集成；
consumer protocol/Host/language 合同不属于 framework Issue #106。

## 来源与范围

回归入口位于 finalize.rs，compact/tools/stream_channel 是同一 safe-point 影响闭包。

## 已知缺口

最终 focused/full gate 由 framework-only closure evidence 绑定；跨仓 E2E 不属于本 Finding。
