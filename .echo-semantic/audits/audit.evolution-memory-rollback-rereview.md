---
schema_version: 1
id: audit.evolution-memory-rollback-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: data_durability
freshness: examined
revision: a7d72961ad4418c7e0eb3255cf377042940b4eab
finding_refs: [finding.evolution-changelog-rollback-authority]
challenges:
  canonical-target-and-generation-fence:
    revision: a7d72961ad4418c7e0eb3255cf377042940b4eab
    source_refs: [src/evolution/layer.rs, src/evolution/mutation.rs, src/evolution/review.rs]
    evidence_refs: [evidence.evolution-memory-rollback-repair, evidence.evolution-memory-rollback-verification]
  inverse-projection-and-audit-lineage:
    revision: a7d72961ad4418c7e0eb3255cf377042940b4eab
    source_refs: [src/evolution/layer.rs, src/evolution/audit.rs]
    evidence_refs: [evidence.evolution-memory-rollback-repair, evidence.evolution-memory-rollback-verification]
  restart-idempotency-and-scope-boundary:
    revision: a7d72961ad4418c7e0eb3255cf377042940b4eab
    source_refs: [src/evolution/layer.rs, docs/adr/0065-evolution-memory-audit-reconciliation.md]
    evidence_refs: [evidence.evolution-memory-rollback-repair, evidence.evolution-memory-rollback-verification]
---

# Evolution memory later rollback independent rereview

## 审查范围

复审 canonical layered-memory later rollback 的 public preview/receipt、ChangeId/BatchId
目标解析、完整 merge batch、generation CAS/ABA、non-tip fence、request-id 幂等冲突、
rollback-of-rollback、重启恢复、inverse business audit 与 Skill/Rule/host 分层边界。

## 已检查故障假设

- merge member 只恢复单 key，留下部分回退；
- 同名 key 经后续变化或 ABA 后仍被旧请求覆盖；
- 重试创建第二批 inverse，或同 request ID 静默绑定不同 target；
- inverse ChangeLog 类型、warm/hot projection 或 lineage 与真实结果不一致；
- audit 失败或进程重启后丢失 receipt，或重复业务 audit；
- memory slice 被误宣称为 Skill、Rule 或 host rollback 的完整关闭。

## 实际实现路径与证据

`MemoryLayerManager` 从 canonical operation journal 解析目标，merge 任一成员扩展为完整
batch，并在写入前比较每个 key 的最新 generation。inverse 作为新 durable batch 进入既有
prepare -> projection -> idempotent audit -> settlement/reconcile 路径；稳定 request ID
返回原 receipt，冲突 target 失败关闭，rollback-of-rollback 形成新 lineage。Create/Delete、
Update、Promote/Demote 的反向类型和 exact warm/hot before/after projection 均由矩阵验证。

最终复审锚点为 `a7d72961`，基准为 `origin/main@577e0b8c`，完整 diff SHA256 为
`ed06dc54a1dcc803d5c4e239af68a73da1635a95613a42cd3e895c8b7072d19b`。
两套 Clippy、all-target/all-feature workspace tests、no-default workspace check、16 项
独立 feature check、formatter、文档契约与 strict semantic snapshot/change-evidence 均通过。

## 问题记录

最终独立复审未发现 Critical、Important 或 Minor 问题；未新增 Finding。

## 残余风险

raw Store 读者在 reconcile 前可暂见 prepared 中间态；显式压缩或遗失 operation journal
后不能 rollback。全历史扫描尚无 bounded checkpoint，物理断电未做硬件级注入。
Skill/Rule/host rollback 仍未交付，因此 Finding #52 保持 open。

## 未检查项

未执行真实断电、journal compaction、第三方 Store 故障矩阵或 EKO host approval policy。
本 Audit 不证明 remote-main delivery。
