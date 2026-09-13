---
schema_version: 1
id: finding.background-review-detached-persistence-settlement
kind: finding
type: implementation_bug
status: open
severity: medium
primary_focus: failure_concurrency
focus: [time_lifecycle, data_durability, result_side_effect]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.eval-evolution.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Background Review丢handle后持久化无人结算

## 问题

BackgroundReviewer返回JoinHandle且文档允许discard；任务可auto-persist memory，失败只写ReviewOutcome，丢handle后success/panic/write failure均无人观察，max_iterations也未消费。

## 触发条件与影响

调用方认为review已启动即可继续或shutdown时，后台持久mutation可能晚到、失败或被截断且无receipt/debt。

## 证据

`src/evolution/background_review.rs`的spawn、discard合同、auto-persist和review_and_wait差异提供证据。

## 处理记录

Failure Audit确认；后续repair提供owned task/receipt/cancel/deadline和持久mutation settlement。
