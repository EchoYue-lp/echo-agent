---
schema_version: 1
id: evidence.compacted-context-projection-repair
kind: evidence
observed_at: 674f2b41fd1bd25baedbf7102b26565f2771391d
source_refs:
  - echo-state/src/compression/mod.rs
  - src/agent/snapshot.rs
  - src/state/mod.rs
  - src/state/file.rs
  - src/state/sqlite.rs
  - src/agent/react/capabilities.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/context.rs
  - src/agent/react/run/phases/compact.rs
  - src/agent/react/tests.rs
  - docs/adr/0056-durable-transcript-projection-settlement.md
  - docs/en/41-persistence-concepts.md
  - docs/zh/41-persistence-concepts.md
supports: [finding.compacted-transcript-retained-ordinals, finding.canonical-root-system-refresh, behavior.context-memory-lifecycle, rule.context-persistence-separation]
limitations:
  - Local focused framework qualification only; full six gates, 17 features, refreshed SDK Host/contract, website sync and exact main CI remain separate
  - New native persistence cases use File stores; no new claim of per-backend or per-language matrix coverage
  - Raw logs, source copies and binaries are ignored local artifacts available only in this worktree; hashes are identities rather than substitutes for originals
  - Legacy payload versions 1/2 do not prove compression-origin metadata; external MemoryPromoter effects are not transactionally undone
command_results:
  - {"command": "cargo test -p echo_agent --lib managed_compaction --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo test -p echo_agent --lib context_binding --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo test -p echo_agent --lib canonical_root --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo test -p echo_agent --lib runtime_cursor_is_stable --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo test -p echo_state --lib context_transform --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo test -p echo_state --lib canonical_root --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo clippy -p echo_agent -p echo_state --lib --tests --locked -- -D warnings", "exit_code": 0}
  - {"command": "cargo test -p echo_agent --lib managed_compaction_checkpoint_preserves_retained_transcript_ordinals --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo fmt --all -- --check", "exit_code": 0}
  - {"command": "cargo test -p echo_agent --lib canonical_root_agent_refresh_and_setters_keep_derived_system_and_scope --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo clippy -p echo_agent -p echo_state --lib --tests --locked -- -D warnings", "exit_code": 0}
  - {"command": "cargo test -p echo_agent --lib canonical_root_agent_refresh_and_setters_keep_derived_system_and_scope --locked -- --nocapture", "exit_code": 0}
  - {"command": "cargo clippy -p echo_agent -p echo_state --lib --tests --locked -- -D warnings", "exit_code": 0}
  - {"command": "cargo fmt --all -- --check", "exit_code": 0}
---

# Compacted context and transcript identity: bounded framework repair

## 来源与范围

The retained-ordinal defect was present in the f990 production cursor/coordinator.
A real ReactAgent with FileConversationStore and FileRuntimeStateStore first
settled user/tool-call/tool-result/final at 0–3, evicted only the final, then
force-checkpointed. The corrected natural before ran one test and failed when
four durable rows became seven: retained 0/1/2 were copied to 4/5/6.
Its raw is 6,049 bytes / SHA-256
`0a0f69a3042c5b1550798bcb1a10c5acd7668b7768a9c1118906fccca17cec51`.
The exact unchanged test-file after1 ran one pass, raw 862 bytes /
`7ac90e71dfb24dabc880b5f2187e384ba8dd04051e419d40aed6e6271904bf2a`.
The earlier 15,731-byte before1 also observed that real assertion failure before
an unexecuted postcondition parser mistake; it remains separate historical raw.

The extended control then exposed a different existing f990 root-System bug:
Horizon correctly stored Global Objective + root prompt in the checkpoint, but
resume's working-directory refresh replaced the first System entry and lost the
objective. The raw was 3,403 bytes /
`4e3fbcbcfbdc820440017b2cc1aff116c63c7cad4d9db7a0f8e37adb67066280`,
with three native passes, this product failure, and a separate observer-count
oracle failure. The root repair reuses the existing canonical prompt identity.
The Horizon testcase body, enabled objective, eviction and full restore equality
were unchanged for its after run: five native cases passed, raw 969 bytes /
`7c64f6939a6540c9bfc5cfc7bb6b288057662569f256a50501d05c3fd7644dcd`.

The later strengthened existing AgentRoot case exposed a value-selection
subcause in that same canonical-root invariant. Its current getter correctly
kept the active trait override, but working-directory refresh rendered the raw
base. Natural before raw was 1,035 bytes /
`2330a30cdb542ff8969f3b82470896b20bda93235e332e014a280f722fb62a95`:
one failure at the cwd context-root assertion; later inherent/replace/reset
postconditions had not executed. The exact same whole test file then passed
all sequence assertions, raw 548 bytes /
`ae4ecd06f7f909a9dac32c8a04e1856a8f2c0a41c0162853de295cefecac7d02`.
The private renderer now consumes the original override-first current value and
retains CoT, profile suffix and project-rule order. Base setters and skills do
not clear the override; the borrowed getter remains base-only; replace is the
explicit clearing operation. Reset accepts canonical metadata plus its final
rendered view together, preserving its original scope-clear/hook behavior.

## Authority and failure policy

ContextManager retains the active model view; RuntimeStateStore retains the one
checkpoint/cursor; ConversationStore retains original append-only facts.
The strict `projected` ordinal frontier remains authoritative. Sparse
`context_only` positions/digests explicitly identify transformed view occurrences
without assigning a fake transcript ordinal. Original facts settle before
compression, and full transformed messages/bindings share the original CAS.
New same-text turns append normally; an empty active view retains the frontier.

Prepared context state captures messages/scopes/accepted metrics and defers
installed and temporary observers. It stages the coherent public pre-view and
releases Context during Store I/O, supporting actual read-only reentrancy.
Success publishes the frozen tuple/version without another await. Errors and
dropped futures restore the volatile tuple, invalidate hydration and reject
unsafe managed writes until authority reload. Recovery never replays the
compressor/promoter to reconstruct state; a CAS timeout/cancel may already have
committed and is not a durable rollback. The representative post-commit response
cut verifies that actual authoritative reload consumes the stored derived view.

Canonical root updates use the existing exact pure System payload, preserve
other System messages and projection scopes, reuse a desired root already
present, and notify once only for actual canonical message changes. The
standalone no-canonical first-System contract remains unchanged. Root refresh,
trait/inherent prompt setters and skill injection share the same update owner.
Root occurrence identity and effective value selection are both required; they
are separate subcauses in the one canonical-root Finding.

## 支持的结论

Twelve distinct tests executed: ten new regressions and two existing controls.
They cover the tail case; real Horizon non-contiguous retained facts, derived
view restart and empty frontier; horizon-only prepare without primary stats;
promoter-await cancellation; read-only Store reentry and commit-before-response
cancellation; bound schema3/legacy/downlabel/corruption; pending cursor-after and
original lineage; canonical-root setter/desired/missing/legacy cases; retained
projection scopes/accepted metrics/observer deferral; and the existing actual
incremental-summary observation-cache rejection control. The original repeated
cursor/generation control also passed. The tail case was rerun after a Clippy
fixture representation fix and is not added again to the distinct-test count.

The first focused Clippy rejected only the tail fixture's unnecessary Vec
(`10,169B / e116661c426084417715b707f21b43f429455b7afa798b474a9989b955bbb7c2`).
Replacing its outer four-message Vec with an array preserved elements, iteration
and assertions; the exact before2/after1 source/raw remain preserved. The position-epoch
Clippy used the same argv and passed (`350B /
1cae38ff0c94c54896f7d68fbada875ce88d7544f2a1585d2aa095e46563ec86`).
Final array-case tail raw is `546B /
f0d7a11d01dc1dc357805fdb2aaa6712d06128780076c4463e48fed8360e2431`.
The new root-setter fixture's earlier trait/inherent method-resolution compile
failure executed zero tests, was fixed with explicit inherent UFCS, and is not
classified as another product defect. No warnings were suppressed.

Commands used one Cargo process, jobs=1, incremental=0, dev/test debug=0 and
strip=symbols. Assertions, overflow checks and unwind retain their ordinary
profiles; no RUSTFLAGS or non-resource profile override was added. Final source
guards had no drift or resource interruption. The preserved position-epoch proof contains
nine successful command receipts and 12 source/doc identities; peak own target
was 2,574,352,384 bytes and minimum free was 14,541,942,784 bytes. Its 12 distinct
test qualifications retain that source epoch. The later strengthened AgentRoot
sequence is the same existing test, not a thirteenth distinct regression.
Its final fixture explicitly disables project-rule discovery to isolate
base/override/Objective/scope assertions: the production renderer retains rules.
This is default-feature-equivalent fixture isolation, not all-feature proof.
Two stale explanatory comments were corrected. Final selector raw is 548 bytes /
`9e3951c4bf6cf36fa1f637da930e5df827c03d44861ca5ffa1288928971c01b5`,
final focused Clippy raw is 200 bytes /
`ded19caf2200c0186057f8793bb829ada1151e7902555ef7e7f1249af4c55d32`,
and final fmt check is empty raw / exit 0. Latest value proof freezes 13 current
source/doc paths (10 Rust, three docs) and links the original focused proof.
Historical exact test-file identity does not apply to this later fixture cut.

## 已知缺口

The original independent framework reader reviewed the retained-view design,
then the root helper/effective-value renderer, source/runtime results and
fixture-only array/project-rule-isolation deltas:
C0/I0/M0. Earlier contiguous-window and plain-LCS approaches were rejected before
production; they are not reported as successful fixes. Current source identity is
`source:b2639597dfa94b7e16aa312e46db6585d9df7636d6340c004ae978f18711d0f1`; repair and verification are bound to this same source and to
`audit.context-memory.data-durability`'s two current challenges. Each Finding is
resolved only for its local focused framework invariant. Broad Behavior status
is not promoted, and there is no new SDK/main/full-gate claim.

The accepted Rust aggregate checkpoint shape changes and uses internal payload
schema3, so old framework readers fail closed instead of ignoring provenance.
New fields/types are not automatically new SDK operations; the original accepted
aggregate requires normal canonical-generator compatibility review, appropriate
Host pin and lossless current-value proof. Those follow-up consumer obligations
remain with the SDK owner. No new remote DTO or parallel Store is introduced.

Originals remain only in `.echo-semantic/local/transcript-compaction-projection/`
(`before2-proof.json`, `before-root-proof.json`, `after1-proof.json`,
`focused-final-proof.json`, `before-root-value-proof.json`,
`after-root-value-proof.json`, `root-value-final-proof.json`, continuity notes
and their logs/source/binaries).
They are local inspection artifacts, not tracked semantic objects or code refs.
The actual machine preflight base is f990; the baseline's f3fa lineage is retained.
Prior e3 and e3-to-f990 qualifications keep their original ranges. The installed
0.5 full strict route validates this current slice/history; the known task-reader
historical SDK/512-closure limitation and native Stop status are not relabeled.
