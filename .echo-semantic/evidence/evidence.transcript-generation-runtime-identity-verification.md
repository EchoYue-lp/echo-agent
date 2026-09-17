---
schema_version: 1
id: evidence.transcript-generation-runtime-identity-verification
kind: evidence
observed_at: source:bfa5b4590c617d8286f2b80571d5c450622d47c7425d6f1ecb1978bf85743352
source_refs:
  - echo-core/src/agent/mod.rs
  - src/agent/snapshot.rs
  - src/agent/react/run/stream_channel.rs
  - src/state/mod.rs
  - docs/adr/0001-channel-session-sender-scope.md
  - docs/en/41-persistence-concepts.md
  - docs/zh/41-persistence-concepts.md
supports: [finding.transcript-generation-runtime-identity, behavior.context-memory-lifecycle, rule.context-persistence-separation]
limitations:
  - remote Linux and Windows CI remain pending until the pull request is opened
  - ConversationStore projection settlement remains tracked independently by finding.transcript-projection-settlement
command_results:
  - { command: "cargo test -p echo_agent transcript_generation_runtime_identity --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent invocation_runtime_state_identity_controls_restore_and_save --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent transcript_projection_cursor_round_trips_and_rejects_corruption --locked", exit_code: 0 }
  - { command: "cargo clippy -p echo_agent --lib --tests --locked -- -D warnings", exit_code: 0 }
  - { command: "./scripts/verify.sh", exit_code: 0 }
  - { command: "semantic strict snapshot and base change-evidence", exit_code: 0 }
---

# Runtime state 与 transcript generation identity 验证证据

## 支持的结论

实现前两个回归测试均失败，分别观察到 mismatch 被 admission 接受和 checkpoint 成功写入；实现后
两项均通过。Admission 用例在 execution mutex 被占用时仍在 100ms 内返回 typed RuntimeState
错误，并证明 guard、input lifecycle、trace、context、LLM 与 RuntimeStateStore 均无副作用。

Direct snapshot 用例证明 `save_runtime_checkpoint` 在读取 context 或写 Store 前拒绝 mismatch，且
没有 checkpoint 或 scope binding。既有 A/A restore/save、product conversation fallback、
`transcript_generation_id=None` 调用和损坏 cursor 恢复拒绝保持兼容。

## 来源与范围

Focused fmt、Clippy、checkpoint/stream tests 和完整 `./scripts/verify.sh` 均退出 0。完整门禁包含
workspace all-target/all-feature 两档 Clippy、全部测试与 benches，以及 workspace lib no-default check。

## 已知缺口

本 Evidence 不覆盖 #106 的 transcript backend failure、timeout、ambiguous commit 和 durable debt。
