---
schema_version: 1
id: evidence.background-review-application-repair
kind: evidence
observed_at: 6fd66621e0671028d6aa94c69933b63f38e299cd
source_refs:
  - src/evolution/background_review.rs
  - docs/adr/0058-background-review-settlement-ownership.md
  - docs/adr/0065-evolution-memory-audit-reconciliation.md
supports: [finding.background-review-detached-persistence-settlement, behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - EKO owns the admission journal, Review Inbox, workspace generation, and shutdown lifecycle in its separate repository
  - A process crash cannot return the original in-process outcome; recovery reports an interrupted result with memory attribution when no durable outcome exists
---

# Background Review caller-owned repair

## 支持的结论

Framework main `1927a5fc` exposes a lazy `BackgroundReviewHandle`, pre-poll
`ReviewIdentity`, tri-state persistence status, and an identity-bound Memory
journal. The framework does not spawn a detached review or create a competing
receipt authority. Its merged Git tree equals the seven-check CI candidate
`15c053c0`.

EKO application main `d65c47a` owns the corresponding accepted operation in
`echo-agent-app-core/src/evolution/review_integration.rs` and
`background_review_owner.rs` ([CLI PR #5](https://github.com/EchoYue-lp/echo-agent-cli/pull/5)).
It writes a confirmed `Admitted` receipt before polling the lazy handle and
retains the exact workspace generation. The supervisor owns the child task,
cancellation, shutdown join, and the ordering `Outcome -> Review Inbox ->
Terminal`. The durable receipt holds the pre-admission Memory key fingerprint;
restart reconciles the framework journal and either replays the exact outcome
into the idempotent Inbox or records an interrupted result with true, false,
or unknown memory attribution. A pending identity blocks a second admission.

GUI, TUI, and CLI ordinary Review calls converge on this app-core owner. GUI
exposes the operation ID and read-only receipt; GUI-only Side Conversation
layout remains unrelated to the shared Review capability.

## 来源与范围

Framework source and ADR 0058 define the reusable handle/identity contract;
ADR 0065 defines Memory journal reconciliation. EKO PR #5 and its signed main
commit supply the application admission, shutdown, Inbox, and receipt owner.

## 已知缺口

An outcome that was never durably recorded cannot be reconstructed from a
crashed process. The owner reports uncertainty and permits a new explicit
review only after the old admission has reached a durable terminal; it never
claims rollback or fabricates a candidate.
