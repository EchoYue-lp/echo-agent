---
schema_version: 1
id: audit.skill-lifecycle-authority-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: data_durability
freshness: examined
revision: 37b6908cb2c4884139970c8aed0a0c42bfb366c7
finding_refs: [finding.evolution-changelog-rollback-authority, finding.evolution-skill-promotion-audit]
challenges:
  authority-binding-and-recovery:
    revision: 37b6908cb2c4884139970c8aed0a0c42bfb366c7
    source_refs: [src/evolution/skill_mutation.rs, src/evolution/audit.rs, src/evolution/curator.rs]
    evidence_refs: [evidence.skill-lifecycle-authority-repair, evidence.skill-lifecycle-authority-verification]
  approval-and-inverse-lineage:
    revision: 37b6908cb2c4884139970c8aed0a0c42bfb366c7
    source_refs: [src/evolution/skill_mutation.rs, src/evolution/draft.rs, src/evolution/merge.rs, src/evolution/patch.rs]
    evidence_refs: [evidence.skill-lifecycle-authority-repair, evidence.skill-lifecycle-authority-verification]
  concurrency-path-and-secret-fences:
    revision: 37b6908cb2c4884139970c8aed0a0c42bfb366c7
    source_refs: [src/evolution/skill_mutation.rs, src/evolution/candidate.rs, src/agent/snapshot.rs]
    evidence_refs: [evidence.skill-lifecycle-authority-repair, evidence.skill-lifecycle-authority-verification]
---

# Skill lifecycle mutation authority independent rereview

## 审查范围

复审 Draft、Promote、Touch、Deprecate、Merge、Patch 与 later rollback 的唯一 authority，覆盖
exact SKILL.md bytes、Curator state、approval、business audit、restart reconciliation、Agent usage
adapter、Rule HostOwned 边界和 candidate/Skill 共享 Curator 并发。

## 已检查故障假设

- pending operation 被另一个 ChangeLog destination 接管或首次 bind 写入双 marker；
- rollback 的 non-tip generation 检查与 prepare 之间发生 ABA/TOCTOU；
- prepared inverse retry在未settled时误报完成；
- candidate在Skill文件投影后插入无关Curator entry，导致永久reconcile debt；
- relative、parent traversal或symlink alias绕过per-resource generation；
- exact before/after bytes、历史secret或无效UTF-8进入公开业务日志或SKILL.md；
- approval只存在私有journal，或rollback在ChangeLog中缺机器可读lineage；
- reserved authority marker被合法skill key碰撞并破坏reopen；
- runtime usage创建未知Active或绕过唯一authority。

## 实际实现路径与证据

SkillMutationAuthority绑定一个具有canonical durable identity和reserved marker的ChangeLog，首次
binding与reopen校验在共享journal serial内完成。所有mutation共享同一Arc authority，按
prepare -> canonical file projection -> affected-Skill Curator merge-CAS -> idempotent secret-safe
audit -> settlement/reconcile提交。Rollback在同一serial内解析target、校验generation并写入fresh
inverse；unsettled retry先恢复。Business audit公开digest-bound approval和inverse lineage，exact
bytes仅保留在private journal。Rule mutation无framework owner，返回typed HostOwned。

四轮独立实现复审依次发现并闭合ChangeLog rebinding、rollback TOCTOU、unsettled retry、
Curator跨authority并发、secret leakage、path alias、approval lineage与reserved marker碰撞等问题。
最终候选与advancing-base增量复审均为0 Critical、0 Important、0 Minor。Authority 24/24、
Evolution 209/209、examples 19/19、documentation 12/12通过；最终head通过完整
`./scripts/verify.sh`、17-feature matrix、两套Clippy、no-default、formatter、diff-check与strict
semantic change-evidence。

PR #147七项CI全部通过，squash结果以GitHub verified commit
`37b6908cb2c4884139970c8aed0a0c42bfb366c7`进入远端main。post-merge closure复审未发现
Critical、Important或Minor问题。

## 问题记录

最终独立复审未发现未解决问题；未新增 Finding。

## 残余风险

确定性restart/fault injection不等同物理断电。Rule persistence、host approval UI和产品策略由
embedding host拥有，不由framework从ChangeLog推断。

## 未检查项

未执行真实断电、host Rule持久化或EKO UI端到端；这些不属于framework已声明能力。
