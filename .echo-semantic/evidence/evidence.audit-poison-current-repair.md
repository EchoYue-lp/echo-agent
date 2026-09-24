---
schema_version: 1
id: evidence.audit-poison-current-repair
kind: evidence
observed_at: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
source_refs:
  - echo-state/src/audit/memory.rs
supports: [finding.in-memory-audit-successful-drop, behavior.observation-persistence]
limitations:
  - In-memory admission success does not promise process durability
  - This repair evidence does not establish a full current-branch merge gate or remote Issue closure
---

# Issue 61 poisoned audit lock repair

## 支持的结论

`InMemoryAuditLogger` 的 log/query/snapshot/clear 读取 poisoned `RwLock` 后取得内部 guard，
恢复容器访问；`log` 不再在 poisoned 状态静默丢事件却返回成功。此修复只针对仍有效的
内存容器不变量，不扩大为持久化承诺。

## 来源与范围

修复由 `3735f7e0` 进入 framework main；在 `origin/main@f7c1fef7` 再查同一
`echo-state/src/audit/memory.rs` 实现与故障注入回归。范围仅为 in-memory 实现。

## 已知缺口

当前分支的定向测试、完整门禁与独立复审另由各自证据记录；本文件不代替这些收据。
