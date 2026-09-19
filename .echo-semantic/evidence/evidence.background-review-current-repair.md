---
schema_version: 1
id: evidence.background-review-current-repair
kind: evidence
observed_at: source:e0466fe1f836ebe116374a7941ec3b4edbab8b0c9346247efa66405841a178f9
source_refs:
  - src/evolution/background_review.rs
  - docs/adr/0058-background-review-settlement-ownership.md
  - docs/en/25-self-improvement.md
  - docs/zh/25-self-improvement.md
supports: [finding.background-review-detached-persistence-settlement, behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - Current worktree implementation is uncommitted and its source digest is not frozen
  - EKO admission, workspace lease, shutdown drain and UI are owned by a separate application repository
  - Process abort cannot produce an in-process settlement receipt
---

# Issue 38 background review repair candidate

## 支持的结论

Framework `BackgroundReviewer::review`、`review_and_wait` 与 `review_by_run_id` 已改为惰性
async operation，返回 `ReviewOutcome`，不自行 spawn detached task。未 poll 的 future 不触及
RunStore、LLM 或 memory。自动持久化以 `ReviewCandidate.persisted` 三态区分已确认提交、未尝试
与写入尝试后结果未知；未知结果保留 candidate/key，不宣称 rollback 或盲重试。`max_iterations=0`
在 effect 前拒绝，panic 归入可观察 error outcome。

## 来源与范围

Framework 只拥有一次 review 与结果；背景 admission、workspace lease、shutdown 与 evidence inbox
属于 EKO 应用 owner。ADR 0058 记录选项、决策和进程终止限制。本证据不能证明应用分支已完成，
也不能以框架候选取代应用验收。

## 已知缺口

跨仓组合门禁和最终独立复审未完成；进程 abort 无 in-process 收据。
