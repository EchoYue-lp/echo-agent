---
schema_version: 1
id: evidence.skill-candidate-audit-repair
kind: evidence
observed_at: source:d0d70f0819595200d148564ce11cb55de583ba943d7a5554b4c06a0f6dcb8af9
source_refs:
  - src/evolution/candidate.rs
  - src/evolution/curator.rs
  - src/evolution/audit.rs
  - docs/adr/0068-skill-candidate-mutation-audit-reconciliation.md
supports: [finding.skill-candidate-reinforcement-audit-gap, behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - Remote-main delivery and post-merge closure rereview remain pending
  - Promotion, approval, draft, merge, patch and public Skill rollback remain outside this repair
---

# Issue 94 Skill candidate audit repair candidate

## 支持的结论

首个确定性回归已在 `c1f209b3` 基线上复现：create 后 ChangeLog 为 1 条，新增观察触发
reinforcement 后仍为 1 条，测试在预期 2 条处失败。候选设计让 create 与 reinforce 共用私有
candidate operation journal，按 prepare、TypedMemoryStore projection、Curator create registration、
stable `ChangeLog::record_idempotent`、settled 的顺序结算；detect 起点先 reconcile。

独立复审发现 path-only journal binding、get-then-put 与无 lineage Curator registration 三个缺口。
修订候选增加 Store/ChangeLog reserved authority marker、Store exact atomic compare-and-put 与 Curator
candidate authority lineage。已有 journal 遇到空/不同 marker 拒绝，CAS mismatch 不覆盖外部更新，
同 lineage Draft/Active 合法而无 lineage 或不同 lineage 同名 Skill 冲突关闭。

第三轮独立复审发现 Curator 的 `with_extension` 派生会让同 stem 的不同 state extension 共用
journal、sidecar、lock 与临时文件。最终候选对完整 state path 追加 suffix，保持路径单射。

## 权威与范围

TypedMemoryStore 继续拥有 candidate JSON payload，Curator 继续拥有 Skill lifecycle，ChangeLog
继续是 append-only business audit。通用 durable append/replay 复用 `FileEventJournal`；不会把
Skill 状态塞入只理解 warm/hot memory 的 `MemoryOperationJournal`，也不新增 public rollback API。

## 来源与范围

来源覆盖 candidate create/reinforce 主路径、Store public CAS、三个原子内置 Store、明确拒绝
CAS 的 EmbeddingStore、Curator 私有 lifecycle lineage、ChangeLog stable identity 与 ADR 0068。
范围只包含 candidate payload mutation 与对应 audit/reconcile；不接管 promotion/approval policy。

## 已知缺口

完整分支门禁与独立实现复审已通过；当前材料仍不证明 PR CI、remote-main delivery 或
post-merge closure rereview，因此 Finding 保持 open。
