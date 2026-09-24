---
schema_version: 1
id: evidence.audit-poison-current-verification
kind: evidence
observed_at: bd17c73075d6b3cf8e00877fa0fb10d36694ea54
source_refs:
  - echo-state/src/audit/memory.rs
supports: [finding.in-memory-audit-successful-drop]
limitations:
  - The isolated full local gate passed on e8371e58; PR/CI, remote main, and downstream consumers remain unverified
command_results:
  - { command: "cargo test -p echo_state poisoned_write_lock_recovers_log_query_snapshot_and_clear --locked", exit_code: 0 }
  - { command: "cargo test -p echo_state --all-features --locked", exit_code: 0 }
  - { command: "CARGO_TARGET_DIR=/Users/ls/MyWork/code/ylp_agent_learn/lp-agent/echo-agent/.worktrees/issue-61-83-70-38-evidence/target CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 ./scripts/verify.sh", exit_code: 0 }
---

# Issue 61 verification frontier

## 支持的结论

源码回归 `poisoned_write_lock_recovers_log_query_snapshot_and_clear` 注入 poison 后覆盖四条
公开路径。当前 `origin/main@f7c1fef7` 集成源码的 focused 命令退出 0，1 passed、0 failed。
此前 `echo_state` all-features package suite 356 unit 与 17 doctest 通过，
poisoned-lock 回归明确为 `ok`。退出码 0；日志：
`observation-state-all-feature-rerun-1789724292605.log`。
证据分支 `e8371e58` 以独立 `CARGO_TARGET_DIR` 执行 `./scripts/verify.sh` 退出 0，
覆盖 fmt check、两档 workspace Clippy、workspace all-target/all-feature 测试与
no-default-features library check。

## 来源与范围

源码引用为 memory.rs；当前 focused、此前 package suite 与 `e8371e58` 完整门禁
分属不同执行轮次。完整门禁结果由主任务在本隔离工作树运行并回报，target 未纳入 Git。

## 已知缺口

独立复审另见 `audit.audit-poison-lock-rereview`。CLI/SDK 消费者、PR/CI、远端 main
与 Issue #61 验收尚无最终收据。
