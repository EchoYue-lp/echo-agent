---
schema_version: 1
id: evidence.channel-driven-turn-verification
kind: evidence
observed_at: source:4b8f4328fa7758990ed6a070317da3dc5435f74f29f6f8f3f4e4ae5454da5afe
source_refs:
  - src/channels.rs
  - echo-integration/src/channels/session.rs
  - docs/en/15-im-channels.md
  - docs/zh/15-im-channels.md
supports: [behavior.agent-turn-lifecycle, rule.turn-terminal-authority]
limitations:
  - Full all-feature workspace gate and independent rereview remain required before merging and closing Issue 107
  - Existing Issue 134 source-digest evidence and repository baseline require integration refresh after this source change
---

# Channel driven Turn verification

## 支持的结论

Focused `cargo test -p echo_agent --features channels --lib channels::tests
--locked` passed all eight channel tests. They exercise a real `ReactAgent` with
mock provider: receipt identity, final answer, provider usage, completed
delivery, normal outbound projection, failed provider execution, and a cancelled
Turn that cannot produce a successful reply, a failing projection sink, and a
Session reset that cancels and settles a real active Turn before cleanup.
`cargo test -p echo_integration --features channels channels::session::tests
--locked` passed all 26 session tests, including the fail-before-fix driven
setup settlement case. `cargo check -p echo_agent
--no-default-features --features channels --locked` passed the independently
selected channels feature. `cargo clippy -p echo_agent --lib
--no-default-features --features channels --locked -- -D warnings
-D clippy::unwrap_used -D clippy::expect_used -D clippy::panic
-D clippy::unreachable`, `cargo fmt --all -- --check`, and `git diff --check`
also passed. Semantic strict/change-evidence confirmed this Finding's mapping,
but remains red on the separate #134 source-digest records and baseline refresh.

## 来源与范围

The commands ran on the `fix/Echoyue/issue-107-channel-turn` worktree based on
`0415ba15eb8d348f357fe55df4448897677e6960`. Coverage is scoped to the
framework channel adapter and standalone channels feature.

## 已知缺口

Full merge gates, the unrelated #134 source-digest refresh, independent
rereview, and remote main delivery are outstanding.
