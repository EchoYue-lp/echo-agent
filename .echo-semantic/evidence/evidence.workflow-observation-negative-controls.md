---
schema_version: 1
id: evidence.workflow-observation-negative-controls
kind: evidence
observed_at: f3fa0705f62fe813a5a86bc41cd673f82f915a61
source_refs:
- echo-orchestration/src/workflow/graph.rs
- echo-orchestration/src/workflow/node.rs
- echo-orchestration/src/workflow/checkpoint_store.rs
- docs/adr/0088-subagent-settlement-observation.md
- docs/supreme/specs/2026-10-14-subagent-physical-settlement/design.md
supports:
- behavior.task-subagent-execution
- rule.task-subagent-authority
- rule.turn-terminal-authority
limitations:
- Two new named Graph scenarios use the same MemoryCheckpointStore instance; they do not add File, restart, remote Store or power-loss evidence.
- All six local workspace gates passed on the frozen code; exact merged-main CI and independently owned SDK/CLI pin qualification remain separate.
- No production code, public API, state authority, checkpoint format or accepted SDK contract changes.
---

# Workflow source observation negative controls

## 来源与范围

Base `6563d26570cc4a8d93c86b7e37443b0559476807`; only the existing cfg(test)
live-source fixture in graph.rs changes. Production Graph, Node and Store blobs
are identical across663cf644,9886502 and6563d265. The existing Audit's historical
Graph-specific foreign/latestUnknown coverage limitation remains an accurate
statement about that earlier slice; these two named tests fill that narrow gap.
Core/Driver tests or the earlier Graph45 result were not used as a substitute.

## 支持的结论

`workflow_live_source_foreign_observation_survives_matching_progress_and_local_terminal`
feeds foreign Unknown, matching Unknown, local FinalAnswer and real EOF into the
original Node raw source. It returns the original foreign run/execution identity
and retains the same protected Graph claim with no replacement or second Agent
invocation. Matching progress cannot discharge another physical authority.

`workflow_live_source_late_matching_unknown_requires_a_new_terminal` feeds
matching Unknown, matching terminal, later matching Unknown and EOF. The last
Unknown retains the same original claim; an earlier terminal is not its receipt.
The original matching-observation/later-terminal/EOF positive control retains its
existing single-Agent-node path and still settles the original checkpoint.

Both negatives keep the original backend and EOF gates, require pending receipt
and zero source drops while EOF is held, then one real source drop and one Agent
call after release. A test FunctionNode captures the same resumed SharedState
before the original AgentNode; no lock crosses await. Its result key and assistant
messages remain absent, separately from the unchanged checkpoint snapshot in the
same Memory Store. This is test observation, not another execution or state owner.

Actual focused command:
`cargo test -p echo_orchestration --lib workflow_live_source_ --locked -- --nocapture`.
Result: 5 passed, 0 failed, 0 ignored, 505 filtered. Full raw6227 bytes,
SHA-256 `afd76086bdb9b4f353375512d1acc3c39a6469ee389100fe2037a650373f03f9`.
Then `cargo test -p echo_orchestration --lib workflow:: --locked -- --nocapture`
passed93 / failed0 / ignored0 / filtered417; raw8192 bytes, SHA-256
`6d92ddc31a0193d857e62ca811c686779e880438e6a4be0180e1f740006aca7d`.
Both runner receipts exit0 without truncation, warnings or Rust/Cargo input drift.
The Graph source SHA-256 is
`6f2d67d97c1b3c7ba4e82d686c2d3bbbff8c235fe67d739b3ed74cbd008c0335`.

## 已知缺口

No behavioral red occurred and production was not changed. The historical broad
Finding remains open for independently owned cross-repository acceptance; two
negative controls do not close that whole Finding or global semantic coverage.
Independent candidate-preparation and post-execution result reviews both passed
with zero Critical/Important/Minor findings. Exact checkpoint semantic qualification
and file necessity are verified separately before its normal commit/push.
All raw, input manifests, resource samples and test patch remain ignored in
`.echo-semantic/local/workflow-observation-negative-controls/`.

The cold focused run peaked at572833792 target bytes with jobs1/debug0/
incremental0/strip and free space stayed above16994947072 bytes. This measured
focused peak replaces only this slice's earlier rough scheduling estimate, not a
bound for full all-feature gates. The coordinator owns later heavy-slot scheduling.

The accepted c0bc validator separately checks the clean exact Git checkpoint
candidate with `--strict-snapshot`, then `--strict-snapshot --base 6563d265...
--require-change-evidence`. This is source-tree qualification, not an installed
0.5 native Hook/Stop receipt or new main acceptance. Tested and final source
snapshots differ only in thin current documentation; all1023 Rust/Cargo inputs,
including the full Graph fixture, remain byte-identical to the executed controls.

## 完整本地门禁

All six repository workspace gates passed on the same frozen1023 Rust/Cargo
inputs: `cargo fmt --all`, fmt check, all-target/all-feature Clippy with warnings
denied, production four panic-class Clippy, the full all-target/all-feature
locked tests, and the workspace/lib/no-default-feature locked check. Each raw
receipt exits0 without warning, truncation or input drift. These are fresh
results for the two added Graph tests, not reused from the earlier ACP delivery.

Full tests: 87 top-level harness summaries, 3629 passed / 0 failed / 3 existing
live ignored tests (installed LSP, scholarly providers, Zotero credentials).
Two `echo_state` child-process harnesses each passed one test with382 filtered;
they are not counted again in the top-level total. Both new Graph named cases
actually report `ok` in the full suite. Raw379245 bytes, SHA-256
`449a49eeaee20112e8d130d4684061f1af2fafa5ee76a5bfc0c7981aa87e8258`.
Full-test peak target7087173632 bytes, minimum free9486630912 bytes; neither
the9GiB target bound nor the5GiB free floor was reached.

No manifest, feature topology, public API or production branch changed, so the
standalone feature matrix is not triggered by this test-only slice. Current
documentation and semantic metadata are qualified on the final exact Git tree;
all1023 executed Rust/Cargo inputs remain byte-identical. Formal source references
use main ancestor6563 or the final current source snapshot, so no historical
source lookup depends on deleting this task branch or a PR-only commit.
