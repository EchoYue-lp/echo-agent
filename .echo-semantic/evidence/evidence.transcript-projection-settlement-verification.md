---
schema_version: 1
id: evidence.transcript-projection-settlement-verification
kind: evidence
observed_at: source:bfa5b4590c617d8286f2b80571d5c450622d47c7425d6f1ecb1978bf85743352
source_refs:
  - echo-core/src/error.rs
  - echo-core/src/memory/conversation.rs
  - echo-state/src/memory/file_conversation.rs
  - echo-state/src/memory/sqlite_conversation.rs
  - src/state/mod.rs
  - src/state/file.rs
  - src/state/sqlite.rs
  - src/agent/react/tests.rs
  - src/agent/react/run/stream_channel.rs
  - src/agent/react/run/phases/finalize.rs
  - src/agent/snapshot.rs
  - docs/adr/0056-durable-transcript-projection-settlement.md
supports: [finding.transcript-projection-settlement, behavior.context-memory-lifecycle, behavior.observation-persistence, rule.context-persistence-separation, rule.fact-projection-separation]
limitations:
  - Full workspace merge gate and public feature matrix run after the branch merges latest main
  - Remote Linux and Windows CI remain pending until the pull request is opened
  - SDK verification is explicitly outside this framework evidence
command_results:
  - { command: "cargo test -p echo_agent --lib transcript --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent state:: --lib --features sqlite --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent state::checkpoint_tests --lib --locked", exit_code: 0 }
  - { command: "cargo check -p echo_agent --lib --locked", exit_code: 0 }
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
  - { command: "git diff --check", exit_code: 0 }
---

# Transcript projection durable settlement verification

## 支持的结论

Conversation File/SQLite tests 覆盖 epoch acquire/recreate/overflow、atomic merge、ordinal conflict、managed
mutator fence、deadline queue/authority lock 与无副作用失败。Runtime File/SQLite tests 覆盖 revision CAS、
attempt dispatch/result、proof ack、lost ack、scope/generation fence、retention floor、corrupt state 与 restart。

Agent focused tests 覆盖 store-only 和缺 deadline capability admission、retiring/deleted/unbound scope、
checkpoint-only compatibility、cross-runtime hydration、guard/hook/final intervention、direct/stream identity、
force checkpoint、max iteration hook ordering、settlement-before-terminal 与 single observation provenance。
`transcript` filter 最终 25/25；Runtime state suite 58/58；checkpoint suite 15/15。三个 backend/contract
切片均由独立 reviewer 复核，最终集成复审结论为 Critical 0、Important 0、Minor 0。

## 来源与范围

验证在 `fix/Echoyue/transcript-projection-settlement` framework worktree 执行；formatter、focused tests、
root lib check 与 diff check 全部退出 0。完整 workspace/all-feature/no-default/feature matrix 在合入最新
main 后只运行一次，并将在最终交付前追加到本 Evidence。

## 已知缺口

当前证据不代表 SDK parity 或远端平台 CI 已完成，因此不能关闭 GitHub Issue #106。
