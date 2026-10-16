---
{
  "schema_version": 1,
  "id": "evidence.command-cell-cancel-artifact-settlement-repair",
  "kind": "evidence",
  "observed_at": "7110a27e7b0e9841dcfa0d4cca1cdbc053d96177",
  "source_refs": [
    "echo-orchestration/src/tasks/command_cell.rs",
    "docs/adr/0025-deterministic-command-cell-watcher.md"
  ],
  "supports": [
    "behavior.task-subagent-execution",
    "behavior.effect-permission-execution",
    "rule.permission-effect-order"
  ],
  "limitations": [
    "Current7110 native observation only; original unavailable history, old executions and old reviews are not reconstructed or reexecuted.",
    "Current source/test/limit mapping is in evidence.framework-history-current-reobservation; full05 is existing execution, not a newly run historical command."
  ]
}
---

# Current observation and historical record

The machine header now records only the bounded native observation at `7110a27e7b0e9841dcfa0d4cca1cdbc053d96177`; current source/tests and limitations are in `evidence.framework-history-current-reobservation`. Original observation `f44fcb47c31668ec32104096fa0a729e75a1d39a` and its complete metadata remain in [the immutable875 document](https://github.com/EchoYue-lp/echo-agent/blob/875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c/.echo-semantic/evidence/evidence.command-cell-cancel-artifact-settlement-repair.md). The original execution counts, before/after claims and review prose below are preserved verbatim as historical statements, not rerun or equivalence claims for7110.


# CommandCell cancellation/artifact settlement repair

## 支持的结论

`supervise_prepared_cell` 继续作为 CommandCell 唯一终态发布者；artifact finalizer 现在同时监听 manager shutdown、cell cancellation、owner cancellation 和原始绝对 deadline。任一取消在 finalizer 阶段获胜时，finalizer 记录 typed `CommandCellArtifactStatus::Failed`，supervisor 再发布 `Cancelled` terminal state。

普通 `stop`、owner cancellation 与 shutdown 都沿同一 bounded finalization 路径，不创建第二个终态或清理器。

## 来源与范围

`owner_cancellation_aborts_blocking_artifact_finalizer` 与 `stop_aborts_blocking_artifact_finalizer` 使用受控 finalizer hook 证明阻塞 finalizer 会被相应取消唤醒；已有 shutdown、deadline、artifact failure 测试继续覆盖其它中断来源。

## 已知缺口

未执行跨平台进程组、文件系统故障注入、loom/stress 和真实慢文件系统验证；这些边界保留给后续专项审计。

## 处理记录

对应 Finding #44；修复提交为 `19dab55d1017b444585798984676e578a3b0db24`，竞态测试补充提交为 `f44fcb47c31668ec32104096fa0a729e75a1d39a`。
