---
schema_version: 1
id: evidence.compression-token-tail
kind: evidence
observed_at: e5372b8ca3dc308ce8dde4a0092ea58b8d7dd21d
source_refs: [echo-core/src/compression.rs, echo-state/src/compression/compressor/sliding_window.rs, echo-state/src/compression/compressor/summary.rs, echo-state/src/compression/compressor/hybrid.rs, echo-state/src/compression/mod.rs, echo-state/src/compression/verifier.rs, src/config.rs, src/agent/react/capabilities.rs, src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/tools.rs, src/agent/snapshot.rs, echo-agent-learning/examples/demo05_compressor.rs, docs/adr/0081-token-budgeted-compression-tail.md, docs/en/04-compression.md, docs/zh/04-compression.md]
supports: [asset.context-manager, behavior.context-memory-lifecycle, rule.context-persistence-separation]
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
