---
schema_version: 1
id: audit.background-review-application-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: failure_concurrency
freshness: examined
revision: 6fd66621e0671028d6aa94c69933b63f38e299cd
finding_refs: [finding.background-review-detached-persistence-settlement]
challenges:
  before-poll-admission:
    revision: 6fd66621e0671028d6aa94c69933b63f38e299cd
    source_refs: [src/evolution/background_review.rs, docs/adr/0058-background-review-settlement-ownership.md]
    evidence_refs: [evidence.background-review-application-repair, evidence.background-review-application-verification]
  partial-memory-write-and-owner-loss:
    revision: 6fd66621e0671028d6aa94c69933b63f38e299cd
    source_refs: [src/evolution/background_review.rs, docs/adr/0065-evolution-memory-audit-reconciliation.md]
    evidence_refs: [evidence.background-review-application-repair, evidence.background-review-application-verification]
  post-poll-receipt-failure:
    revision: 6fd66621e0671028d6aa94c69933b63f38e299cd
    source_refs: [src/evolution/background_review.rs, docs/adr/0058-background-review-settlement-ownership.md]
    evidence_refs: [evidence.background-review-application-verification]
---

# Background Review application-owner independent rereview

## 审查范围

Independent read-only review of EKO CLI PR #5 and its final candidate found no
remaining implementation blocker. The review traced pre-poll admission,
generation retention, cancellation and shutdown join, `Outcome -> Inbox ->
Terminal`, exact-key Memory reconciliation, and restart recovery. It
specifically required and then rechecked real owner-path fault injection for
Outcome and Terminal append failures. The merged CLI main tree equals the
reviewed and CI-validated candidate tree.

## 已检查故障假设

An observer can disappear after Memory mutation but before the ReviewOutcome;
a journal append can fail after polling; Inbox projection can succeed before
Terminal persistence; an earlier Memory value can be mistaken for this
operation's write; a second generation can attempt to recover another one's
debt. The final candidate fails closed or records explicit uncertainty in
these paths. It does not introduce a second framework supervisor.

## 实际实现路径与证据

Framework `BackgroundReviewer` returns a lazy identity-bearing handle.
EKO's `ReviewGenerationLease::track_review_operation` persists admission before
spawning the child, then its supervisor records Outcome, projects the Inbox,
and records Terminal. On reopen, the receipt journal and framework Memory
journal determine which effect is confirmed or unknown. The repair and
verification Evidence above bind these paths to the merged trees and tests.

## 问题记录

The reviewer initially blocked the candidate on missing real post-poll
Outcome/Terminal append fault tests. Both tests were added through the actual
supervisor path and passed; a final read-only rereview reported no remaining
implementation finding for #38.

## 残余风险

The independent code review passed with zero remaining findings for this
boundary. Exact CLI main CI was checked separately in run 36293620412;
it is not inferred from this rereview. Process crash before a durable Outcome
remains an interrupted/unknown result, not a synthesized success.

## 未检查项

The reviewer did not operate a real Tauri window or execute CI. The separate
mainline run and its result are recorded in Verification Evidence.
