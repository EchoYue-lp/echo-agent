---
schema_version: 1
id: evidence.background-review-application-verification
kind: evidence
observed_at: 1486d2f4a26adbefab393679c58da2804e5f4a75
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

For the reopened 2026-10-11 entry defects, CLI candidate `b34df102` passed its
final focused suite 94/94 and desktop adapter suite 7/7. Both independent scoped
reviews returned pass with zero blockers. The reviewed source and honest limits
are recorded in its
[entry audit](https://github.com/EchoYue-lp/echo-agent-cli/blob/b34df102e8aca5d787c23d851f9c85bc748521ff/.echo-semantic/audits/audit.background-review-entry-independent-rereview.md).
Complete local CLI gates passed: fmt/check, two strict Clippy commands,
workspace all-feature tests (main app-core 1,611 passed / 9 ignored and CLI
285 passed, integration/doctests zero failures), app-core no-default, GUI binary
check and GUI tests (216 library + 1 Tauri entry + 6 JSONL passed, 3 ignored
doctests), 59 document pairs / 49 ADRs, and strict semantic snapshot/change
evidence. Nested subprocess results are not counted twice.

[CLI PR #11](https://github.com/EchoYue-lp/echo-agent-cli/pull/11) passed all four
checks in [run 38105523574](https://github.com/EchoYue-lp/echo-agent-cli/actions/runs/38105523574)
and squash-merged into signed, verified main
`f57b90a2b0962fa5561f8839a3df70be1f261ffa`. Its tree
`6968f8c0b3c7d77a1629419eb693eb5b5cf42733` exactly matches reviewed/tested
candidate `b34df102`. Strict semantic snapshot validation passed on the actual
merged commit. Exact-main [run 38106297341](https://github.com/EchoYue-lp/echo-agent-cli/actions/runs/38106297341)
passed all four checks with run conclusion success; it is not inferred from PR
CI. Transport failures interrupted monitoring, and the final PR/main results
were independently confirmed through REST.

The framework proof candidate keeps runtime code at main `1486d2f4`. Its
`cargo fmt --all` and complete `scripts/verify.sh` exited 0; the latter includes
fmt check, both strict Clippy commands, workspace/all-target/all-feature tests
and workspace/lib no-default compilation. The untruncated receipt is
`review38-framework-proof-final-gate-1791686088430.log`. Strict semantic snapshot
and change-evidence checks also passed. Only semantic evidence is changed, so
the independent feature matrix and example/API documentation changes do not
apply; examples are already covered by the all-target gate.
Every reported suite had zero failures; existing ignored tests remained ignored.

## 已知缺口

The real Tauri window and other embedding applications are not covered by
this verification. GitHub Issue #38 is closed only after this resolved Finding
and its repair, verification, and rereview evidence reach framework main.
The new tests do not drive a native window, Tauri restart/setup panic or a real
provider, and do not establish full cleanup of early post-fork initialization
errors. Historical PR #5/main CI does not substitute for the new delivery.
