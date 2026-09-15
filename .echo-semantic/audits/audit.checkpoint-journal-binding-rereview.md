---
schema_version: 1
id: audit.checkpoint-journal-binding-rereview
kind: audit
boundary_ref: boundary.observation-persistence-delivery
lens: data_durability
freshness: examined
revision: dcd25a8a19c21c3247965e527684d89c96d68ac7
finding_refs: [finding.checkpoint-journal-binding]
challenges:
  foreign-checkpoint-and-generation:
    revision: dcd25a8a19c21c3247965e527684d89c96d68ac7
    source_refs: [echo-state/src/journal/mod.rs, echo-state/src/journal/file.rs]
    evidence_refs: [evidence.checkpoint-journal-binding-repair, evidence.checkpoint-journal-binding-verification]
  pruned-segmented-identity:
    revision: dcd25a8a19c21c3247965e527684d89c96d68ac7
    source_refs: [echo-state/src/journal/mod.rs, echo-state/src/journal/segmented.rs]
    evidence_refs: [evidence.checkpoint-journal-binding-repair, evidence.checkpoint-journal-binding-verification]
  receipt-and-schema-boundary:
    revision: dcd25a8a19c21c3247965e527684d89c96d68ac7
    source_refs: [echo-state/src/journal/mod.rs, echo-state/src/journal/file.rs, echo-state/src/journal/segmented.rs, docs/adr/0055-checkpoint-journal-identity.md]
    evidence_refs: [evidence.checkpoint-journal-binding-repair, evidence.checkpoint-journal-binding-verification]
---

# Checkpoint 与 Journal generation 绑定独立复审

## 审查范围

复审`JournalIdentity`在Memory/File/Segmented authority中的分配与持久生命周期，batch/checkpoint/retention marker摘要，CheckpointStore与recover顺序，apply_committed receipt来源，schema v1拒绝策略、公共trait调用点、ADR/双语文档和Finding状态。

## 已检查故障假设

验证同sequence异源checkpoint是否仍可返回Loaded；同路径Journal换代是否复用旧generation state；prefix prune后是否错误尝试从缺失事实重建；外部committed receipt是否能先fold再伪装成本地checkpoint；File或Segmented合法frame能否混入另一generation；删除identity的v1数据是否会被猜测接受；marker或checkpoint identity篡改是否逃逸digest。

## 实际实现路径与证据

Journal batch、append receipt、checkpoint与segmented retention marker都携带同一generation identity并进入相应digest。统一fold路径在读取records前比较receipt与当前Journal identity；recover在sequence范围判断前比较checkpoint identity，完整Journal从0重建，pruned Journal失败关闭。File cold scan拒绝mixed frame，Segmented scan在各segment内部及跨segment/marker两层比较identity。

独立源码review、增量测试构造review和最终语义锚点review均pass，Critical/Important/Minor为0。116项Journal focused tests、echo_state/root/ACP direct checks、两档Clippy与fmt全部通过。

## 问题记录

本次复审未发现Issue #43范围内的新反例。修复、验证和独立复审证据均已形成，但Finding按任务要求保持open，等待最终delivery分支integration gate和全局语义归并。

## 残余风险

自定义`EventJournal`或`CheckpointStore`仍可能违反公共trait的identity持久合同；schema v1采用明确拒绝而非迁移；本地验证不替代其它操作系统的文件持久性和完整workspace all-feature门禁。

## 未检查项

未执行完整workspace/all-feature门禁、远端Linux/Windows CI、真实断电fsync测试或最终delivery分支跨任务语义连续性归并。
