---
schema_version: 1
id: evidence.evolution-memory-audit-repair
kind: evidence
observed_at: source:512b2adda3fbd65e8d7e3c2f4d23a036338d495ab4c3b276f09a15af58ed99f9
source_refs:
  - src/evolution/mutation.rs
  - src/evolution/layer.rs
  - src/evolution/review.rs
  - src/evolution/audit.rs
  - src/evolution/runtime_integration.rs
  - src/tools/builtin/memory.rs
  - src/memory_promoter.rs
  - src/agent/react/run/context.rs
  - docs/adr/0065-evolution-memory-audit-reconciliation.md
  - docs/en/25-self-improvement.md
  - docs/zh/25-self-improvement.md
supports: [finding.evolution-audit-atomicity, behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - Independent rereview, full task-branch gates and remote-main delivery are pending
  - Raw Store or MEMORY.md readers can see an intermediate projection until startup reconciliation
  - Later rollback remains tracked by finding.evolution-changelog-rollback-authority
---

# Issue 51 layered memory audit repair candidate

## 支持的结论

`MemoryLayerManager` 对 warm write/delete/meta、hot/warm 迁移、预算降级和 approved merge
在 mutation 前 durable prepare；同一批次保存全部目标值和固定 `ChangeEntry` ID。投影后
`JsonlChangeLog::record_idempotent` 提交业务审计，最后写入 settled fact。失败或进程中断时，
重启从唯一 `echo-state::FileEventJournal` 权威校验历史目标状态，补齐目标投影及未结算审计。
已结算投影也参与校验，以覆盖 Store 报告 degraded durability 后的磁盘回退。

## 来源与范围

热层含换行或首尾空白内容通过frontmatter的`content_json`标记与JSON bullet逐字回读，
避免已settled journal目标和持久投影冲突。晋升/降级、revive与删除将旧值带入
root串行prepare区复核，交错写入时失败而非覆盖新值。

`MemoryMerger` 公共能力保留择主、元数据与结果，但删除独立逐 key Store/audit 循环，真实
`apply_merge_proposal` 主路径改由 manager 提交整个组。工具前缀删除、压缩 promoter、
BackgroundReview 与 Dreaming 仍经 manager，manager 读入口在 pending 或启动恢复前
失败/先恢复。ADR 0065 记录跨文件可见性、同根并发、未知 append outcome 与 API 迁移。

## 已知缺口

这是未提交分支候选，不是独立复审或远端主线交付证明。独立持有底层 Store 的读者不受 manager
围栏保护；若外部写入产生不属于 journal 历史的值，恢复失败关闭而不覆盖。observer 的外部
effect 没有 durable acknowledgement，因此恢复不重放 callback。技能/规则独立 Finding 与
ChangeLog 事后 rollback (#52) 不因本修复自动关闭。
