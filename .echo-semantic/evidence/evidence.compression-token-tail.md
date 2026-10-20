---
schema_version: 1
id: evidence.compression-token-tail
kind: evidence
observed_at: e5372b8ca3dc308ce8dde4a0092ea58b8d7dd21d
source_refs: [echo-core/src/compression.rs, echo-state/src/compression/compressor/sliding_window.rs, echo-state/src/compression/compressor/summary.rs, echo-state/src/compression/compressor/hybrid.rs, echo-state/src/compression/mod.rs, echo-state/src/compression/verifier.rs, src/config.rs, src/agent/react/capabilities.rs, src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/tools.rs, src/agent/snapshot.rs, echo-agent-learning/examples/demo05_compressor.rs, docs/adr/0081-token-budgeted-compression-tail.md, docs/en/04-compression.md, docs/zh/04-compression.md, src/agent/config.rs, src/agent/react/run/phases/prepare.rs]
supports: [asset.context-manager, behavior.context-memory-lifecycle, rule.context-persistence-separation, finding.runtime-token-limit-explicit-publication]
limitations: [Local candidate is not merged; external provider and native application acceptance are not established]
---

# Token-budgeted compression tail

## 支持的结论

The before-change test `summary_preserves_latest_request_before_long_tool_batch`
failed because the exact Chinese request was no longer retained. The repaired
Summary and IncrementalSummary use the same pure selection as SlidingWindow.
Token mode keeps complete older turns, the latest request and atomic tool blocks;
oversized requests fail explicitly. Positive legacy caps remain available.

Automatic compact passes the latest non-projection/runtime-note query as focus.
Manual force variants share protected budget accounting, sanitization, verifier
fallback, canonical reinjection and final allowance checks. Manual provider focus
and cancellation follow one hook/trace owner. No new store or state owner exists.

Tool groups include intervening hook notes and rich-result attachments. Accepted
input messages own incremental history; `context_committed` only publishes an
observation cache after final acceptance. Cancellation and rejected transforms
leave both context and that cache unchanged.

## 来源与范围

The focused compression suite and root/state suites passed after repair
(809 root and 344 state tests). Observer override/restore/clear and near-limit
candidate provenance regressions passed after the final repair. Independent
review returned pass with zero remaining findings. Full lint and panic-API
checks passed; final workspace tests, features and examples are recorded at
handoff without treating this candidate as merged. Historical transcript/memory
audits remain historical; this Evidence does not claim all context-memory open
findings are resolved.

Final `scripts/verify.sh` and the seventeen required independent feature checks
passed on this source snapshot. The full all-target/all-feature workspace run
recorded 3,396 passed, zero failed and three marked doctests ignored. Command
receipt: `framework-outcome-final-fixed-1791561703908.log` in the task worktree's
common Git `supreme/logs` directory. Examples are included in the all-target run.

The all-features canonical model-profile fixture now isolates checkout-local
rules and uses a valid budget. Its original suffix assertion remains, with real
history eviction and final allowance assertions. Its focused before-change
failure and isolated after-change pass are preserved in the command logs;
independent incremental review confirmed that the original pass still applies.

## 已知缺口

External provider, native UI, remote CI and mainline delivery remain outside these local checks.

## Explicit runtime token limit (2026-10-19)

Repair `e3ba883a9abaf4e7070185304ccb676c55292144` adds the existing explicit marker
at the two successful runtime publication points. Its natural framework before
run failed both direct and prepared real-turn compressor observations with
102,400 rather than 80,000; the unset-profile and rejected/dropped controls passed.
The identical four regressions passed after repair. The actual compressor input
plus the same Context's protected-token estimate is asserted before and after
two completed Agent chat turns. Matching existing prepared tests (nine selected,
one overlapping the new tests), focused lib/tests Clippy and fmt passed.
Independent source rereview returned C0/I0/M0. These counts describe this bounded
run, not replacement totals for the older compression suite above.

Raw files remain ignored under `.echo-semantic/local/live-token-limit-explicit/`:
`before-result.json`, `after-result.json`, `clippy-result.json`,
`prepared-regressions-result.json` and their logs. The before log is 11,322 bytes,
SHA256 `0452f9c6e1089dc2ac0c1a029c4b1d5ea0200be8ccd8102d92f83d02b1b71741`;
the after log is 846 bytes, SHA256
`bb7f0e6fca4b04d4218d8a43a81e4cdbca0e71a60dd10dd94a7b0c66b3914514`.
These originals are local to this worktree; hashes identify the recorded files and
do not substitute for executed tests. The new Finding's formal integrated framework
rereview returned C0/I0/M0 and its local invariant is resolved; full workspace/features, precise-main CI and refreshed native
SDK acceptance are separate delivery obligations.
