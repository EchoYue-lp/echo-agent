---
schema_version: 1
id: evidence.channel-attachment-projection-verification
kind: evidence
observed_at: source:e09ea140ba27724c9d3fa92c16a8139087ddfd0256c1f0d614f153e6086b54cd
source_refs:
  - src/channels.rs
  - docs/adr/0077-channel-attachment-projection.md
supports: [finding.channel-attachment-projection]
limitations:
  - Remote CI and mainline delivery remain pending
  - Provider-specific binary file rendering and actual QQ or Feishu media retrieval were not exercised
command_results:
  - { command: "cargo test -p echo_agent --features channels --lib channels::tests --locked", exit_code: 0 }
  - { command: "cargo test -p echo-agent-learning --test documentation_contract --locked", exit_code: 0 }
  - { command: "cargo clippy -p echo_agent --features channels --lib --locked -- -D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable", exit_code: 0 }
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
  - { command: "git diff --check", exit_code: 0 }
  - { command: "./scripts/verify.sh", exit_code: 0 }
---

# Channel attachment projection verification

## 支持的结论

Before the fix, the real handler regression failed because the model received
plain text instead of a typed File. After the fix, all 12 channel tests passed
with no warnings. The new cases inspect a real synchronous model request,
attachment-only streaming, binary File byte round-trip, ordered mixed content,
four image signatures, and pre-model rejection of unrepresented media.

## 来源与范围

The test call records the exact `Message` reaching `MockLlmClient`. Targeted
Clippy, format checking, and all 14 documentation contract tests passed on the
candidate source. The unchanged reviewed source then passed the full workspace
gate with exit code 0, recorded in `issue40-full-gate-1790305798535.log`.

## 已知缺口

Independent rereview found no blocker. Provider wire translation and
transport-specific media acquisition are separate boundaries; remote CI and
mainline delivery are still pending.
