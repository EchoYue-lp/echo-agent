---
schema_version: 1
id: audit.command-cell-settlement-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: time_lifecycle
freshness: examined
revision: f44fcb47c31668ec32104096fa0a729e75a1d39a
finding_refs: [finding.command-cell-cancel-artifact-settlement, finding.command-cell-retention-lease-prune-race]
challenges:
  cancellation-finalizer:
    revision: f44fcb47c31668ec32104096fa0a729e75a1d39a
    source_refs: [echo-orchestration/src/tasks/command_cell.rs]
    evidence_refs: [evidence.command-cell-cancel-artifact-settlement-repair, evidence.command-cell-cancel-artifact-settlement-verification]
  lease-aware-removal:
    revision: f44fcb47c31668ec32104096fa0a729e75a1d39a
    source_refs: [echo-orchestration/src/tasks/command_cell.rs]
    evidence_refs: [evidence.command-cell-cancel-artifact-settlement-repair, evidence.command-cell-cancel-artifact-settlement-verification]
---

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
