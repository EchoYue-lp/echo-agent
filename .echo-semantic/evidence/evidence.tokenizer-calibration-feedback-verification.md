---
schema_version: 1
id: evidence.tokenizer-calibration-feedback-verification
kind: evidence
observed_at: source:5e9a01f48cdf8338bbf8ccfdf225290e1c3f3db90fbea6c30cf1219b92cd1f48
source_refs:
  - echo-core/src/tokenizer.rs
  - echo-state/src/compression/mod.rs
  - src/agent/react/run/phases/compact.rs
  - src/agent/react/run/phases/think.rs
  - src/agent/react/run/stream_channel.rs
supports: [finding.tokenizer-calibration-feedback-convergence]
limitations:
  - Remote CI and mainline delivery remain pending
  - Three existing ignored tests were not executed by the full gate
command_results:
  - { command: "cargo test -p echo_agent image_ --lib --locked", exit_code: 101 }
  - { command: "cargo test -p echo_agent schema_overhead_compacts_history_before_model_admission --lib --locked", exit_code: 101 }
  - { command: "cargo test -p echo_state prepare_without_budget_reserves_request_overhead --lib --locked", exit_code: 101 }
  - { command: "cargo test -p echo_agent format_only_compression_flushes_draft_before_eviction --lib --locked", exit_code: 101 }
  - { command: "cargo test -p echo_agent agent::react::run::phases::think::tests --lib --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent agent::react::run::phases::compact::stage4_e1_tests --lib --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent schema_overhead_compacts_history_before_model_admission --lib --locked", exit_code: 0 }
  - { command: "cargo test -p echo_state prepare_without_budget_reserves_request_overhead --lib --locked", exit_code: 0 }
  - { command: "cargo clippy -p echo_core -p echo_state -p echo_agent --lib --tests --locked -- -D warnings", exit_code: 0 }
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
  - { command: "git diff origin/main --check", exit_code: 0 }
  - { command: "./scripts/verify.sh", exit_code: 0 }
  - { command: "for feature in acp a2a mcp lsp sqlite telemetry topology subagent web media data statistics channels git database rag chart; do cargo check -p echo_agent --no-default-features --features \"$feature\" --locked || exit 1; done", exit_code: 0 }
  - { command: "uv run /Users/ls/MyWork/code/ylp_agent_learn/echo-semantic/skills/semantic-contract/scripts/verify_semantic.py --root /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/tokenizer-calibration-feedback --strict-snapshot", exit_code: 0 }
  - { command: "uv run /Users/ls/MyWork/code/ylp_agent_learn/echo-semantic/skills/semantic-contract/scripts/verify_semantic.py --root /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/tokenizer-calibration-feedback --strict-snapshot --base d7c6aff4c0fddba010bb8528724185d4095893c8 --require-change-evidence", exit_code: 0 }
---

# Tokenizer feedback production-path verification

## 支持的结论

On the pre-fix candidate, the image-budget regression produced 2,195 tokens
instead of 1,110 after text calibration. A text-image-text production sequence
also accepted the image response as a second calibration sample. The real
Agent schema-history test emitted no `ContextCompressed` event, the
no-budget `ContextManager` test returned `compressed: None`, and a
format-only compression run made zero pre-compaction extraction calls rather
than one. Each focused command above exited 101 for its asserted behavior.

## 来源与范围

After repair, the think-phase tests pass 10/10, including repeated raw
feedback convergence, cache-aware/missing usage, fixed image cost, and
changing tool visibility. The memory-flush tests pass 8/8, including a real
`MemoryLayerManager` Draft written before format-only compression. The
schema-history Agent test passes with both percentage budget enabled and
disabled, compacts before admission, and makes one model call. The direct
no-budget context test passes. Focused Clippy, formatting, and diff checks
exit zero on merge snapshot `a96ae92a86864449f47e02cc5e0048041fce0337`.
Strict-snapshot and change-evidence checks against `main@d7c6aff4` also exit
zero after the current source digest was refreshed.

The exact integrated snapshot passed `./scripts/verify.sh` from 2026-09-25
10:01:35–10:10:54 UTC. Its untruncated 375,238-byte log is
`.git/worktrees/tokenizer-calibration-feedback/supreme/logs/issue100-full-gate-1790330495738.log`:
86 test-result summaries report 0 failed, with 3 existing ignored tests.
The independent feature matrix ran 17 distinct `cargo check` commands from
10:12:56–10:18:25 UTC, each with a `Finished` result and overall exit 0;
its untruncated 23,141-byte log is `issue100-feature-matrix-1790331176061.log`.

## 已知缺口

Tests use a deterministic mock provider; they do not prove an external
provider's hidden tokenizer or exact multimodal pricing. The independent
reviewer confirmed the integrated implementation and semantic diff but did
not run the full gate; the gate and feature matrix are separate command
evidence. Remote CI, mainline delivery, and Issue closure remain pending.
