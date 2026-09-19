---
schema_version: 1
id: audit.evolution-memory-audit-atomicity-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: data_durability
freshness: examined
revision: cb4ee9ed3826fd8055f027e84f69b94dbc329267
finding_refs: [finding.evolution-audit-atomicity]
challenges:
  prepare-projection-audit-settlement:
    revision: cb4ee9ed3826fd8055f027e84f69b94dbc329267
    source_refs: [src/evolution/mutation.rs, src/evolution/layer.rs, src/evolution/audit.rs]
    evidence_refs: [evidence.evolution-memory-audit-repair, evidence.evolution-memory-audit-verification]
  restart-and-settled-projection-recovery:
    revision: cb4ee9ed3826fd8055f027e84f69b94dbc329267
    source_refs: [src/evolution/layer.rs, echo-state/src/journal/file.rs, docs/adr/0065-evolution-memory-audit-reconciliation.md]
    evidence_refs: [evidence.evolution-memory-audit-repair, evidence.evolution-memory-audit-verification]
  concurrent-and-multi-key-authority:
    revision: cb4ee9ed3826fd8055f027e84f69b94dbc329267
    source_refs: [src/evolution/mutation.rs, src/evolution/layer.rs, src/evolution/review.rs, echo-state/src/journal/file.rs]
    evidence_refs: [evidence.evolution-memory-audit-repair, evidence.evolution-memory-audit-verification, evidence.foundation-36-72-51-integration-verification]
  raw-store-and-rollback-boundary:
    revision: cb4ee9ed3826fd8055f027e84f69b94dbc329267
    source_refs: [src/evolution/layer.rs, src/evolution/runtime_integration.rs, docs/adr/0065-evolution-memory-audit-reconciliation.md]
    evidence_refs: [evidence.evolution-memory-audit-repair, evidence.evolution-memory-audit-verification]
---

# Evolution memory audit atomicity independent rereview

## 审查范围

复审 layered-memory durable prepare、Store/MEMORY.md projection、幂等业务 audit、
settlement、重启恢复、已结算投影回退、同根并发、跨进程 lease 与 approved multi-key merge。

## 已检查故障假设

- prepare 失败后仍修改投影；
- audit 或 settlement 失败后留下永久无审计 mutation；
- 已结算 Store 投影回退后恢复误认为完成；
- 两个 manager 或进程交错覆盖，或 stale decision 覆盖较新值；
- multi-key merge 半途失败后只恢复部分成员或重复业务 audit；
- raw Store 中间态或 later rollback 被误宣称已闭合。

## 实际实现路径与证据

操作日志在任何投影前持久 prepare 完整 before/after 和固定 ChangeEntry ID。同根
mutation/reconcile 共用串行锁，FileEventJournal 持有跨进程 writer lease。live settlement
按 projection、idempotent audit、Settled 顺序执行；重启扫描完整历史，把每个 key 前滚到
最新目标，并只为未结算 batch 重放固定 audit ID。Approved merge 使用单一 prepared batch。
主线 commit 与已验证 PR head tree 相同，PR #138 七项 CI 全绿。

## 问题记录

未发现 Critical、Important 或 Minor 问题；未新增 Finding。

## 残余风险

raw Store/MEMORY.md 直接读者可暂见 prepared 中间态，须在发布前完成 reconciliation。
显式 NullChangeLog 是测试/feature-disabled opt-out，不提供 durable business audit。observer
callback 无 durable acknowledgement，恢复不重放。journal 保留与全历史扫描仍无 bounded
checkpoint，物理断电未做硬件级注入。

## 未检查项

未执行真实断电、长时间跨进程 stress、journal compaction 或第三方 Store 故障矩阵。
ChangeLog later rollback 与逐项 restore 仍由 finding.evolution-changelog-rollback-authority
和 Issue #52 负责；skill/rule audit Finding 不在本次关闭范围。
