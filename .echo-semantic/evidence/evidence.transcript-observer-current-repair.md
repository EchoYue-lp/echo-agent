---
schema_version: 1
id: evidence.transcript-observer-current-repair
kind: evidence
observed_at: source:3131a2a66cf2c665575e853a3ae3b43620bafd243e2a60e61fdeaa886c34f846
source_refs:
  - src/agent/react/run/phases/compact.rs
  - src/agent/react/run/phases/tools.rs
  - src/agent/react/run/phases/finalize.rs
  - src/agent/react/run/stream_channel.rs
  - src/agent/snapshot.rs
  - docs/adr/0056-durable-transcript-projection-settlement.md
supports: [finding.transcript-projection-settlement, behavior.observation-persistence, rule.fact-projection-separation]
limitations:
  - Framework settlement fact and stream observer delivery are distinct outcomes
  - SDK protocol, Host and language projections remain cross-repository Issue 106 scope
  - Current source is uncommitted and final regression gate is pending
---

# Issue 106 observer disconnect repair candidate

## 支持的结论

Compact、tool 与 finalize safe point 在获得真正的 store-backed transcript settlement
后，先标记此次结算已经被观察，再尝试向流 consumer 发送事件。关闭的 stream channel
只影响交付观察，不把既已持久化的同一事实误判为需要第二次持久化的失败。底层
pending intent、receipt、CAS 与 managed delete 继续由既有 coordinator 拥有，
此修复不引入第二状态权威。

## 来源与范围

仅修正 framework 安全点的 observation flag 顺序，不改变底层 store-backed coordinator。

## 已知缺口

SDK Host/语言消费和最终源码复验还没有当前轮次的通过收据。
