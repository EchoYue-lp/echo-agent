---
schema_version: 1
id: finding.background-review-detached-persistence-settlement
kind: finding
type: implementation_bug
status: resolved
severity: medium
primary_focus: failure_concurrency
focus: [time_lifecycle, data_durability, result_side_effect]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.background-review-current-repair, evidence.framework-four-finding-counterexample-rereview, evidence.background-review-application-repair, evidence.background-review-application-verification]
audit_refs: [audit.eval-evolution.failure-concurrency]
decision_refs: []
repair_evidence_refs: [evidence.background-review-application-repair]
verification_evidence_refs: [evidence.background-review-application-verification]
rereview_audit_refs: [audit.background-review-application-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Background Review丢handle后持久化无人结算

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/38

## 问题

BackgroundReviewer返回JoinHandle且文档允许discard；任务可auto-persist memory，失败只写ReviewOutcome，丢handle后success/panic/write failure均无人观察，max_iterations也未消费。

## 触发条件与影响

调用方认为review已启动即可继续或shutdown时，后台持久mutation可能晚到、失败或被截断且无receipt/debt。

## 证据

`src/evolution/background_review.rs`的spawn、discard合同、auto-persist和review_and_wait差异提供证据。

## 处理记录

`3735f7e0` 删除了可丢弃的 detached JoinHandle，改为返回 awaited ReviewOutcome；
未 poll 的 future 不产生 effect，持久化错误在 await 返回时可见。当前源码仍允许 caller 在
memory 已写、observer 未完成时 drop review future：`shutdown_can_drain_or_drop_but_drop_does_not_rollback_partial_write`
固定了这一行为，caller 此时拿不到 outcome/receipt。故旧 detached 路径虽已消失，单靠
framework 仍不能闭合持久 mutation 的取消结算缺口；这是本次跨仓修复的原始反例。

当前 framework-only 候选不再引入 detached receipt owner：`BackgroundReviewHandle` 直接由 caller
驱动，首次 poll 前可读取 `ReviewIdentity`（run ID 与 persistence key），且所有失败/unknown
outcome 保留该 identity。应用必须在自己的 generation/admission/shutdown/evidence owner 中保存
identity、驱动或取消 operation，并按 memory journal 重试恢复。EKO CLI 的 caller-owned
consumer 已随 PR #5 合入签名的远端 main `d65c47a`；其 exact main CI run 36293620412
的 Linux Rust 与 frontend 均通过。repair、verification 与独立 rereview 分别绑定对应 Evidence
和 Audit。进程 crash 发生在 durable Outcome 之前时，应用只报告 interrupted/unknown 与
Memory key 归因，不伪造成功；该诚实终态是本 Finding 的闭合合同。
