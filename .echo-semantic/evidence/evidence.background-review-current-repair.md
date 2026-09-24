---
schema_version: 1
id: evidence.background-review-current-repair
kind: evidence
observed_at: 733d352fc719f922b21bab1cd46206139564367f
source_refs:
  - src/evolution/background_review.rs
  - docs/adr/0058-background-review-settlement-ownership.md
  - docs/en/25-self-improvement.md
  - docs/zh/25-self-improvement.md
supports: [finding.background-review-detached-persistence-settlement, behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - Current worktree implementation is uncommitted and its source digest is not frozen
  - EKO admission, generation lease, shutdown drain, evidence inbox and UI are owned by a separate application repository
  - Process abort cannot produce an in-process settlement receipt; only the identity-bound memory journal can be reconciled
---

# Issue 38 background review repair candidate

## 支持的结论

Framework `BackgroundReviewer::review`、`review_and_wait` 与 `review_by_run_id` 返回惰性
`BackgroundReviewHandle`，其 `ReviewIdentity` 在首次 poll 前即可读取。handle poll 直接驱动
caller-owned operation，不自行 spawn detached task、创建 receipt registry 或要求 Tokio runtime。
未 poll 的 handle 不触及 RunStore、LLM 或 memory。既有 `ReviewOutcome.run_id`、persistence
action 和 tri-state candidate 保持向后兼容；不新增必填 public struct 字段。panic、failure、取消
和写入结果 unknown 由 caller 在 settlement 前使用 handle identity 与确定性 key 对账。
自动持久化以 `ReviewCandidate.persisted` 三态区分已确认提交、未尝试与写入尝试后结果未知；未知
结果保留 candidate/key，不宣称 rollback 或盲重试。`max_iterations=0` 在 effect 前拒绝。

## 来源与范围

Framework 只拥有一次 review operation、identity 和 tri-state result；背景 admission、generation
lease、shutdown cancel/drain 与 evidence inbox settlement 属于 embedding application owner。应用
必须在 poll 前保存 identity，并在自己的 owner 中执行 spawn、取消、重试和 evidence settlement。
ADR 0058 记录该边界。本证据不能证明应用分支已完成，也不能以框架候选取代应用验收。

## 已知缺口

跨仓组合门禁和最终独立复审未完成；进程 abort 无 in-process 收据，只有 identity-bound memory
journal 可用于持久 mutation reconciliation。
