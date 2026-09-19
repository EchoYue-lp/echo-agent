---
schema_version: 1
id: evidence.diagnostic-delivery-current-repair
kind: evidence
observed_at: source:b214951ece8e09325efc846ad7bd88a402135000e42fe67d2b917317b2d27923
source_refs:
  - src/trace/mod.rs
  - src/agent/react/mod.rs
  - src/agent/snapshot.rs
  - echo-state/src/audit/file.rs
  - echo-state/src/audit/mod.rs
  - echo-core/src/audit.rs
  - docs/adr/0053-trace-audit-persistence-visibility.md
supports: [finding.diagnostic-persistence-failure-visibility, behavior.observation-persistence, rule.fact-projection-separation]
limitations:
  - This incremental repair does not replace the historical Issue 46 candidate evidence
  - SDK public API inventory and cross-language contract are in another repository and remain open
  - Current source digest and final gates are pending
---

# Issue 46 current consumer and persistence repair candidate

## 支持的结论

FileAuditLogger 的子进程 lease/close-reopen 回归检查互斥边界，文件解码或隔离错误避免
输出原始 payload。Producer 在交给自定义 diagnostic Store/Logger 之前应用 retention，
但诊断持久化失败仍只进入 typed diagnostic delivery、drop counter 与可配置 observer，
不得反向覆盖 Agent 业务终态。新的 `AuditEvent::apply_retention` 公共方法和既有
DiagnosticDelivery API 必须进入独立 SDK inventory，framework 修复不能替代 Host 合同。

`RunStore::finalize_run` 是公开终态 mutation boundary。InMemory 与 JSONL backend 在与
append 相同的排他 mutation authority 下 finalize，保留并发 late event、只接受第一个
terminal，并使 Failed 的 run-level Error 至多一次。trait compatibility default 仍是
load/update/save，不对外部并发 backend 宣称原子性；这类 backend 必须自行 override。

## 来源与范围

Framework FileAuditLogger、RunStore、producer 与诊断 observer 是本 Evidence 范围；SDK 为独立仓库。

## 已知缺口

SDK inventory 必须分类 finalize 的 missing-run boolean receipt；最终源码测试、全门禁与
独立复审均未取得当前轮次通过收据。
