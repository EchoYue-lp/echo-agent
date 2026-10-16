---
{
  "schema_version": 1,
  "id": "audit.command-cell-settlement-rereview",
  "kind": "audit",
  "boundary_ref": "boundary.task-subagent-workflow",
  "lens": "time_lifecycle",
  "freshness": "examined",
  "revision": "7110a27e7b0e9841dcfa0d4cca1cdbc053d96177",
  "finding_refs": [
    "finding.command-cell-cancel-artifact-settlement",
    "finding.command-cell-retention-lease-prune-race",
    "finding.framework-historical-source-observation-retention"
  ],
  "challenges": {
    "current-native-source-and-existing-regressions": {
      "revision": "7110a27e7b0e9841dcfa0d4cca1cdbc053d96177",
      "source_refs": [
        "echo-orchestration/src/tasks/command_cell.rs"
      ],
      "evidence_refs": [
        "evidence.framework-history-current-reobservation"
      ]
    }
  }
}
---

# Current observation and historical record

The machine header now records only the bounded native observation at `7110a27e7b0e9841dcfa0d4cca1cdbc053d96177`; current source/tests and limitations are in `evidence.framework-history-current-reobservation`. Original observation `f44fcb47c31668ec32104096fa0a729e75a1d39a` and its complete metadata remain in [the immutable875 document](https://github.com/EchoYue-lp/echo-agent/blob/875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c/.echo-semantic/audits/audit.command-cell-settlement-rereview.md). The original execution counts, before/after claims and review prose below are preserved verbatim as historical statements, not rerun or equivalence claims for7110.


# CommandCell settlement independent rereview

## 审查范围

复核 CommandCell 的唯一终态发布、artifact finalization cancellation、普通 stop/owner cancellation、terminal retention candidate scan 与 lease-aware removal。

## 已检查故障假设

1. 长 deadline command 在 artifact finalizer 阻塞时，普通 stop 或 owner cancellation 是否仍会长期保持 Running。
2. finalizer 中断是否留下未分类 artifact 状态，或在 command terminal 之前丢失副作用结算。
3. prune candidate scan 后取得的 waiter/observer lease 是否可能被无条件 remove 删除。
4. lease drop 后 retention 是否仍能收敛并删除超出上限的终态 cell。

## 实际实现路径与证据

`finish_output_artifact` 对 shutdown、cell cancellation、owner cancellation 和 deadline 使用同一 `tokio::select!` 边界；`supervise_prepared_cell` 只在 finalizer 返回/记录后写入 terminal state。`remove_terminal_candidate` 使用 DashMap `remove_if` 在 shard 锁内复核 terminal 与两个 lease 计数；新 lease 会阻止删除，lease 释放后现有 Drop/prune 路径继续收敛。

Focused tests、36 项 CommandCell 测试和格式检查均通过。未发现本 Finding 范围内的 Critical、Important 或 Minor 反例。

## 问题记录

本次复审未发现本 Finding 范围内的 Critical、Important 或 Minor 反例；两个 Finding 均具备 repair 与 verification Evidence。

## 残余风险

未覆盖跨平台进程组、loom/stress、真实慢文件系统和 runtime 强制退出；这些不影响本次已验证的 tokio cancellation/select 与 DashMap 原子条件合同。

## 未检查项

未覆盖跨平台进程组、loom/stress、真实慢文件系统和 runtime 强制退出；这些不影响本次已验证的 tokio cancellation/select 与 DashMap 原子条件合同。
