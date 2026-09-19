---
schema_version: 1
id: evidence.audit-poison-current-repair
kind: evidence
observed_at: source:4c4bf7193da92bb6a7830e5a6f769ca18e2ed021db517cbf7050ac8d81769b78
source_refs:
  - echo-state/src/audit/memory.rs
supports: [finding.in-memory-audit-successful-drop, behavior.observation-persistence]
limitations:
  - In-memory admission success does not promise process durability
  - Current uncommitted source and full gate are not yet frozen
---

# Issue 61 poisoned audit lock repair candidate

## 支持的结论

`InMemoryAuditLogger` 的 log/query/snapshot/clear 读取 poisoned `RwLock` 后取得内部 guard，
恢复容器访问；`log` 不再在 poisoned 状态静默丢事件却返回成功。此修复只针对仍有效的
内存容器不变量，不扩大为持久化承诺。

## 来源与范围

范围仅为 `echo-state/src/audit/memory.rs` 的 in-memory 实现。

## 已知缺口

最终源码测试、完整门禁与独立复审待完成。
