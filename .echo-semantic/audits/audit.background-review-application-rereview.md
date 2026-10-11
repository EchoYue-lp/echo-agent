---
schema_version: 1
id: audit.background-review-application-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: failure_concurrency
freshness: examined
revision: 1486d2f4a26adbefab393679c58da2804e5f4a75
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
  application-entry-recovery:
    revision: 1486d2f4a26adbefab393679c58da2804e5f4a75
    source_refs: [src/evolution/background_review.rs, docs/adr/0058-background-review-settlement-ownership.md]
    evidence_refs: [evidence.background-review-application-repair, evidence.background-review-application-verification]
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

Two further independent read-only reviews covered the reopened entry defects in
CLI candidate `b34df102`: desktop return/settlement, real bootstrap recovery and
rollback, shared owner/Store/generation, closed admission, weak projection
fanout, explicit P/Q contexts, idempotent linking, Memory attribution and actual
post-write observer cancellation. Both reviewed the final 94/94 log and returned
pass, zero blocking findings. Lifecycle review diff SHA-256 is
`5f5d8c586f301da4e1d0354265385fd76d8143f00512861dcd47999ae694601a`;
owner/projection review diff SHA-256 is
`48dfea25feeffd412d8bfd0127ede1dcbbf0fa87be3ca30cdf5b546cf9ed7ce7`.
The sole final Clippy syntax adjustment was independently confirmed equivalent.

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
For the new repair, local complete gates and PR CI are green; signed CLI main
`f57b90a2` has the reviewed candidate's exact tree and passed strict semantic
validation. Exact-main CI passed all four checks; the framework-owned evidence
requires its own mainline acceptance. Native window operation, restart/setup
panic and full
cleanup of early post-fork initialization errors remain unproven. The publication
fence's explicit cleanup and durable EffectsApplied retry are the narrower
verified properties.
