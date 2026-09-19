---
schema_version: 1
id: audit.skill-candidate-audit-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: data_durability
freshness: examined
revision: d0d1e97596ecfec4f4489e329b2429c23bd055fe
finding_refs: [finding.skill-candidate-reinforcement-audit-gap]
challenges:
  candidate-payload-and-audit-atomicity:
    revision: d0d1e97596ecfec4f4489e329b2429c23bd055fe
    source_refs: [src/evolution/candidate.rs, src/evolution/audit.rs, echo-core/src/memory/store.rs]
    evidence_refs: [evidence.skill-candidate-audit-repair, evidence.skill-candidate-audit-verification]
  authority-binding-and-external-writer-fence:
    revision: d0d1e97596ecfec4f4489e329b2429c23bd055fe
    source_refs: [src/evolution/candidate.rs, src/evolution/curator.rs, echo-state/src/memory/store.rs]
    evidence_refs: [evidence.skill-candidate-audit-repair, evidence.skill-candidate-audit-verification]
  restart-idempotency-and-path-identity:
    revision: d0d1e97596ecfec4f4489e329b2429c23bd055fe
    source_refs: [src/evolution/candidate.rs, src/evolution/curator.rs, echo-state/src/journal/file.rs]
    evidence_refs: [evidence.skill-candidate-audit-repair, evidence.skill-candidate-audit-verification]
---

# Skill candidate mutation and audit independent rereview

## 审查范围

复审 Skill candidate create/reinforcement 的 payload authority、Curator lineage、Store CAS、
prepare/settle journal、idempotent ChangeLog、重启恢复、路径 identity 与外部覆盖冲突行为。

## 已检查故障假设

- payload 已更新但 audit 丢失，或重启重复业务 audit；
- Store read/CAS 窗口覆盖第三方并发写；
- journal 被绑定到另一个 Store、ChangeLog 或 Curator authority；
- 同名非 candidate lifecycle 被恢复逻辑降级或覆盖；
- derived-index Store 在 payload CAS 后留下半更新 index；
- `state.json` 与 `state.toml` 共用 journal、lineage、lock 或 temporary path；
- no-growth reinforcement 仍产生 Store、journal 或 audit 副作用。

## 实际实现路径与证据

candidate owner 以 private journal 固化 exact before/after、Store 与 ChangeLog identity，并按
prepare -> Store CAS -> Curator lineage -> idempotent audit -> settlement 顺序提交。重启恢复
只对匹配 authority 和 expected state 的操作收敛；未知外部值、authority rebinding 与 lifecycle
冲突均失败关闭。InMemory、File 与 SQLite Store 提供原子 CAS，EmbeddingStore 显式返回
Unsupported，避免 payload/index 半原子更新。所有 sidecar 均向完整 state filename 追加 suffix。

最终实现经过四轮独立复审，结论为 0 Critical、0 Important、0 Minor。最终 head 通过完整
`./scripts/verify.sh`、17-feature matrix、semantic strict/change-evidence、10 轮 `echo_state`
stress 与 30 轮 `echo_execution` stress；PR #143 的 Linux quality、三组 Linux tests、learning、
Windows atomic replacement 和 dependency policy 七项 CI 全绿。修复以 GitHub verified squash
commit `d0d1e97596ecfec4f4489e329b2429c23bd055fe` 进入远端 main。

## 问题记录

post-merge closure rereview 未发现 Critical、Important 或 Minor 问题；未新增 Finding。

## 残余风险

确定性 fault injection 不等同于物理断电。第三方自定义 Store 若不提供原子 compare-and-put，
候选写入会 typed Unsupported。Skill promotion、approval、file mutation 与 later rollback 仍由
Issue #54 负责，不属于本 Finding。

## 未检查项

未执行真实断电、journal compaction 或第三方 Store 的外部故障矩阵；未宣称 #54 已闭合。
