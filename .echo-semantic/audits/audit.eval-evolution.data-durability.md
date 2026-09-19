---
schema_version: 1
id: audit.eval-evolution.data-durability
kind: audit
boundary_ref: boundary.eval-evolution
lens: data_durability
freshness: stale
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.evolution-audit-atomicity, finding.evolution-doc-namespace, finding.evolution-skill-promotion-audit, finding.evolution-changelog-rollback-authority, finding.skill-candidate-reinforcement-audit-gap]
challenges:
  memory-mutation-and-audit:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/evolution/layer.rs, src/evolution/audit.rs, src/evolution/review.rs]
    evidence_refs: [evidence.provider-protocol-quality, evidence.persistence-observation]
  namespace-and-cold-tier:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/evolution/layer.rs, docs/en/25-self-improvement.md, docs/zh/25-self-improvement.md]
    evidence_refs: [evidence.provider-protocol-quality]
  skill-and-candidate-durability:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/evolution/curator.rs, src/evolution/candidate.rs, src/evolution/draft.rs, src/evolution/merge.rs, src/evolution/patch.rs]
    evidence_refs: [evidence.provider-protocol-quality]
---

# Evolution Mutation、Audit 与 Rollback 数据持久性审计

## 审查范围

审查 memory hot/warm mutation、merge/budget、ChangeLog、namespace/cold tier、Skill Curator/draft/merge/patch和candidate reinforcement。

## 已检查故障假设

验证 mutation与audit是否原子/可恢复、文档namespace是否使旧数据不可达、ChangeLog是否真能later rollback，以及Skill/candidate状态是否全部可审计。

## 实际实现路径与证据

Memory write/demote/revive/delete/promote/budget/merge均先改变Store或文件再record audit，失败无durable reconcile；JsonlChangeLog自身原子但生产不使用idempotent recovery。运行时warm namespace是agent/memories，文档仍写typed_memories/默认三层且无迁移；Cold locate分支不可达。Curator多种状态mutation无ChangeLog，candidate已有项reinforcement更新后直接返回。ChangeLog trait没有rollback apply，draft/merge/patch只有同调用best-effort补偿。

## 问题记录

确认三个既有Finding；新增ChangeLog rollback authority与candidate reinforcement audit gap。Cold tier与旧namespace兼容策略、Curator是trusted primitive还是framework-enforced mutation需semantic-decide。

## 残余风险

跨Store/File/Curator需要durable operation identity/reconcile，不应把单个JSONL日志原子性误作跨资源事务。

Issue #51分层记忆repair候选已改变本审查的memory mutation故障假设；本Audit
保留历史revision原结论并标记stale，新的独立rereview须针对候选源码与原始Store可见性重新执行。

## 未检查项

未审查EKO审批adapter、第三方Store故障或真实断电/kill时序。
