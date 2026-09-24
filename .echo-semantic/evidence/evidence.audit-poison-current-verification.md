---
schema_version: 1
id: evidence.audit-poison-current-verification
kind: evidence
observed_at: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
source_refs:
  - echo-state/src/audit/memory.rs
supports: [finding.in-memory-audit-successful-drop]
limitations:
  - Current branch reran the focused counterexample; full workspace gate and remote delivery are pending
command_results:
  - { command: "cargo test -p echo_state poisoned_write_lock_recovers_log_query_snapshot_and_clear --locked", exit_code: 0 }
  - { command: "cargo test -p echo_state --all-features --locked", exit_code: 0 }
---

# Issue 61 verification frontier

## 支持的结论

源码回归 `poisoned_write_lock_recovers_log_query_snapshot_and_clear` 注入 poison 后覆盖四条
公开路径。当前 `origin/main@f7c1fef7` 集成源码的 focused 命令退出 0，1 passed、0 failed。
此前 `echo_state` all-features package suite 356 unit 与 17 doctest 通过，
poisoned-lock 回归明确为 `ok`。退出码 0；日志：
`observation-state-all-feature-rerun-1789724292605.log`。

## 来源与范围

源码引用为 memory.rs；当前 focused 运行与此前 package suite 分属不同执行轮次，
均不代表此证据分支的完整 workspace 门禁。

## 已知缺口

独立复审另见 `audit.audit-poison-lock-rereview`。完整 workspace 合并门禁、CLI/SDK
消费者与远端交付尚无最终收据。
