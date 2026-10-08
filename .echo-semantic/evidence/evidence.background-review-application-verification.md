---
schema_version: 1
id: evidence.background-review-application-verification
kind: evidence
observed_at: 6fd66621e0671028d6aa94c69933b63f38e299cd
source_refs:
  - src/evolution/background_review.rs
  - docs/adr/0058-background-review-settlement-ownership.md
  - docs/adr/0065-evolution-memory-audit-reconciliation.md
supports: [finding.background-review-detached-persistence-settlement]
limitations:
  - A real Tauri window was not used to validate the read-only GUI receipt projection
  - Framework consumers other than EKO must own their own admission and settlement lifecycle
---

# Background Review cross-repository verification

## 支持的结论

- Framework PR [#174](https://github.com/EchoYue-lp/echo-agent/pull/174)
  passed seven remote CI jobs. Signed main `1927a5fc` and tested head
  `15c053c0` have the same Git tree; strict semantic snapshot validation
  passed on the merged framework main.
- EKO CLI PR [#5](https://github.com/EchoYue-lp/echo-agent-cli/pull/5)
  passed its final Linux workspace check, app-core tests (1,594 passed,
  9 ignored), and frontend job at head `6ff9eaf` in
  [run 36292598950](https://github.com/EchoYue-lp/echo-agent-cli/actions/runs/36292598950).
  Signed CLI main `d65c47a` has the same Git tree as that head.
- Exact CLI main `d65c47a` passed both Linux Rust and frontend jobs in
  [run 36293620412](https://github.com/EchoYue-lp/echo-agent-cli/actions/runs/36293620412).
  The Linux app-core suite reported 1,594 passed, 9 ignored, 0 failed;
  the workspace check included fmt, all-target/all-feature Clippy, and
  app-core no-default compilation.
- Local EKO candidate gates passed workspace all-feature tests (app-core
  1,594 passed, 9 ignored; CLI 280 passed), two strict Clippy gates,
  app-core no-default, GUI check/test (211 passed), frontend lint/test/build
  (55 files, 283 tests), bilingual docs parity, and strict semantic validation.
- Real `track_review_operation` fault injection rejects receipt write failure
  before poll; after poll, separate Outcome and Terminal append failures each
  make the observer pass fail, preserve durable debt, join the owner on
  shutdown, and recover exactly once after reopen. Tests also cover a Memory
  write before observer completion, old-key attribution unknown, generation
  mismatch, duplicate admission, and idempotent Inbox replay.

## 来源与范围

Framework PR #174, EKO CLI PR #5, their Git trees and CI runs, and EKO
app-core's real owner-path fault tests provide the recorded verification. The
framework references above identify the reusable half of this cross-repository
contract; the EKO paths and remote runs are linked in the body.

## 已知缺口

The real Tauri window and other embedding applications are not covered by
this verification. GitHub Issue #38 is closed only after this resolved Finding
and its repair, verification, and rereview evidence reach framework main.
