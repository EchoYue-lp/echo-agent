---
schema_version: 1
id: audit.eval-evolution.permission-external
kind: audit
boundary_ref: boundary.eval-evolution
lens: permission_external
freshness: examined
revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
finding_refs: [finding.evolution-skill-promotion-audit, finding.pre-compaction-memory-trust-provenance]
challenges:
  skill-mutation-authorization:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [src/evolution/curator.rs, src/evolution/merge.rs, src/evolution/patch.rs, src/evolution/security.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  background-review-policy:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [src/evolution/background_review.rs, src/evolution/review.rs, src/evolution/runtime_integration.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  pre-compaction-memory-provenance:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [src/agent/react/run/phases/compact.rs, src/agent/react/run/context.rs, src/evolution/recall.rs]
    evidence_refs: [evidence.provider-protocol-quality]
---

# Evolution Approval、Security 与 Trust Provenance 审计

## 审查范围

审查Skill Active/merge/patch授权与security、BackgroundReview proposal/auto-persist，以及pre-compaction自动memory的来源可信度。

## 已检查故障假设

验证高风险Skill mutation是否携带approval/security/audit，proposal-only是否被auto-persist混淆，以及混合transcript是否经LLM重写后失去trust/evidence仍成为Active memory。

## 实际实现路径与证据

Curator promotion/touch可直接Active且无approval/ChangeLog/security；SkillMerger可合入allowed_tools，Patcher写SKILL.md，security check未接生产。BackgroundReviewer默认proposal-only；显式auto-persist只处理高置信、逐字用户preference并写Draft。另一条pre_compaction_flush从user/assistant/tool混合内容生成memory，统一标L3Promotion+Active，Recall只排除Superseded，故会进入后续context。

## 问题记录

扩大Skill promotion Finding到Active/merge/patch统一授权；新增pre-compaction trust provenance。Skill API是trusted-host primitive还是必须验证持久ApprovalArtifact需semantic-decide。

## 残余风险

Framework内未发现Curator Active到真实Skill catalog producer，最终发布影响需应用侧复核；Rule promotion只有未接线security check。

## 未检查项

未检查EKO RulePromoter/ReviewIntegration、Skill命令或外部复用方。
