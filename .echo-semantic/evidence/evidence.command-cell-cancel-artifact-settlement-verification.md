---
schema_version: 1
id: evidence.command-cell-cancel-artifact-settlement-verification
kind: evidence
observed_at: f44fcb47c31668ec32104096fa0a729e75a1d39a
source_refs:
  - echo-orchestration/src/tasks/command_cell.rs
  - docs/adr/0025-deterministic-command-cell-watcher.md
supports: [behavior.task-subagent-execution, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 未执行 echo-agent 合并前完整 workspace 门禁和远端 CI
  - 未执行 loom/stress 或 Windows 进程清理验证
---

# CommandCell cancellation/artifact settlement verification

## 支持的结论

本证据记录本次 CommandCell 修复的工程验证与覆盖范围。

## 来源与范围

来源为 `echo-orchestration/src/tasks/command_cell.rs`、ADR 0025 和以下 focused/unit 命令。

## 已知缺口

未执行 echo-agent 合并前完整 workspace 门禁、远端 CI、loom/stress 或 Windows 进程清理验证。

## 工程验证

- `cargo test -p echo_orchestration owner_cancellation_aborts_blocking_artifact_finalizer --locked`：1 passed
- `cargo test -p echo_orchestration stop_aborts_blocking_artifact_finalizer --locked`：1 passed
- `cargo test -p echo_orchestration retention_rechecks_a_new_lease_before_removal --locked`：1 passed
- `cargo test -p echo_orchestration command_cell --locked`：36 passed, 0 failed
- `cargo fmt --all -- --check`：passed
- `git diff --check`：passed

## 覆盖结论

测试证明 cancel/owner cancellation 不再等待阻塞 artifact finalizer；terminal snapshot 只在 artifact interruption 被 typed 记录后发布。retention 竞态测试证明新 waiter lease 在候选扫描后取得时，`remove_if` 的原子谓词复核保留 cell，lease drop 后才允许移除。

## 处理记录

对应 Finding #44 与 #45；修复提交为 `19dab55d1017b444585798984676e578a3b0db24`，竞态测试补充提交为 `f44fcb47c31668ec32104096fa0a729e75a1d39a`，源码摘要为 `source:e09ea140ba27724c9d3fa92c16a8139087ddfd0256c1f0d614f153e6086b54cd`。
