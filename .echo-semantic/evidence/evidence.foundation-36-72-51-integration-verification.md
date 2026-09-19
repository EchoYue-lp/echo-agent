---
schema_version: 1
id: evidence.foundation-36-72-51-integration-verification
kind: evidence
observed_at: source:16e4824bc89e28e36a6c329505451b8ca5c86d6e4f4d1144ea01616535f3ac09
source_refs:
  - src/acp/adapter.rs
  - src/headless.rs
  - echo-integration/src/channels/session.rs
  - src/plugin/prepared.rs
  - src/evolution/mutation.rs
  - docs/adr/0065-evolution-memory-audit-reconciliation.md
  - docs/adr/0066-agent-adapter-close-ownership.md
supports:
  - finding.agent-adapter-close-settlement
  - finding.plugin-generation-publication-authority
  - finding.evolution-audit-atomicity
limitations:
  - A2A remains untouched and outside the Issue 36 integration slice
  - Remote-main delivery is not established by local receipts
  - Headless outer-future cancellation remains a documented one-shot API limitation
---

# Foundation 36/72/51 integration verification receipts

## 支持的结论

This is the single receipt authority for combined workspace gates, isolated
feature checks, strict semantic validation and final integrated review across
Issues 36, 72 and 51. Focused lane Evidence remains authoritative only for its
own red/green scenarios.

## 来源与范围

The integration owner populates the following table after the last source or
semantic edit. Rows must contain the exact command, exit code and durable log
path; a command run before the final edit cannot be reused.

| Gate | Exact command | Exit | Log or receipt |
| --- | --- | --- | --- |
| Formatter | `cargo fmt --all -- --check` | 0 | `.git/worktrees/foundation-36-72-51/supreme/logs/command-1789805863375.log`; main-agent tool receipt |
| Workspace Clippy | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 0 | `.git/worktrees/foundation-36-72-51/supreme/logs/command-1789805890583.log`; main-agent tool receipt |
| Panic API Clippy | `cargo clippy --workspace --lib --bins --all-features --locked -- -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable` | 0 | `.git/worktrees/foundation-36-72-51/supreme/logs/command-1789805958933.log`; main-agent tool receipt |
| Workspace tests | `cargo test --workspace --all-targets --all-features --locked` | 0 | `.git/worktrees/foundation-36-72-51/supreme/logs/command-1789806008964.log`; 3,088 passed, 0 failed, 3 ignored; main-agent tool receipt |
| No-default check | `cargo check --workspace --lib --no-default-features --locked` | 0 | `.git/worktrees/foundation-36-72-51/supreme/logs/command-1789806260296.log`; main-agent tool receipt |
| Feature isolation | `for feature in acp a2a mcp lsp sqlite telemetry topology subagent web media data statistics channels git database rag chart; do cargo check -p echo_agent --no-default-features --features "$feature" --locked || exit 1; done` | 0 | `.git/worktrees/foundation-36-72-51/supreme/logs/command-1789806275325.log`; 17 feature checks; main-agent tool receipt |
| Semantic strict/change evidence | `uv run /Users/ls/.codex/plugins/cache/echo-semantic/echo-semantic/0.4.0/skills/semantic-contract/scripts/verify_semantic.py --root /Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/foundation-36-72-51 --strict-snapshot --base 39a348b3c9b8a6a7fb3caaa1146832c570e988cb --require-change-evidence` | 0 | `.git/worktrees/foundation-36-72-51/supreme/logs/semantic-final-integration.log`; main-agent tool receipt |
| Final integrated review | independent strict implementation rereview of diff `4f39955d96beff9b5d3a144d8d2141b1ba3c44db0cdaf8fd66243209cd6552e2` and content manifest `fc6da11520e697109f71f7f7bd10a36a935082ca308935d16125f4624bfe1928` | pass | independent reviewer report; 0 Critical/Important findings |

## 已知缺口

The Channel cancellation fix changed production code after the first combined
workspace gate. The main agent reran the affected focused checks, all mandatory
workspace gates, the feature matrix and strict semantic check on the frozen
post-fix snapshot. A2A and remote mainline acceptance remain outside this
Evidence's supported conclusion.
