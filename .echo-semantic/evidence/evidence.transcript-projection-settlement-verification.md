---
schema_version: 1
id: evidence.transcript-projection-settlement-verification
kind: evidence
observed_at: source:df3909bab5e6d047cac27c29ce098a331020e28886a022e3daa383ac12e985f1
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
  - Remote Linux and Windows CI remain pending until the pull request is opened
  - SDK verification is explicitly outside this framework evidence
command_results:
  - { command: "cargo test -p echo_agent --lib transcript --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent state:: --lib --features sqlite --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent state::checkpoint_tests --lib --locked", exit_code: 0 }
  - { command: "cargo check -p echo_agent --lib --locked", exit_code: 0 }
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
  - { command: "git diff --check", exit_code: 0 }
  - { command: "./scripts/verify.sh", exit_code: 0 }
  - { command: "cargo check -p echo_agent --no-default-features --features <each of acp,a2a,mcp,lsp,sqlite,telemetry,topology,subagent,web,media,data,statistics,channels,git,database,rag,chart> --locked", exit_code: 0 }
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

验证在 `fix/Echoyue/transcript-projection-settlement` framework worktree 执行；分支已包含最新
`origin/main@99b9abd6`。`./scripts/verify.sh` 覆盖 formatter、两档 Clippy、workspace all-target/
all-feature tests、examples/learning contracts 与 no-default lib check，最终退出 0；root lib tests
896/896 通过。17 个独立 public feature checks 全部退出 0。

## 已知缺口

当前证据不代表 SDK parity 或远端平台 CI 已完成，因此不能关闭 GitHub Issue #106。
