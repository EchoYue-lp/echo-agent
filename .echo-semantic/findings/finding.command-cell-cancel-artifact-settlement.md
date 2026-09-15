---
schema_version: 1
id: finding.command-cell-cancel-artifact-settlement
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: time_lifecycle
focus: [failure_concurrency, result_side_effect, contract_evidence]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution, behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.task-subagent-workflow, evidence.effects-extensions, evidence.command-cell-cancel-artifact-settlement-repair, evidence.command-cell-cancel-artifact-settlement-verification]
audit_refs: [audit.task-subagent-workflow.time-lifecycle, audit.command-cell-settlement-rereview]
decision_refs: []
repair_evidence_refs: [evidence.command-cell-cancel-artifact-settlement-repair]
verification_evidence_refs: [evidence.command-cell-cancel-artifact-settlement-verification]
rereview_audit_refs: [audit.command-cell-settlement-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# CommandCell 普通 cancel 可卡在 artifact finalization

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/44

## 问题

Stop/owner cancel 令 command 返回 Cancelled 后，artifact finalization 只监听 manager shutdown 与原始命令 deadline，不监听 cell/owner cancellation；terminal 在 finalizer 返回后才发布。

## 触发条件与影响

长 deadline cell 的 artifact flush/sync 卡住时，显式 cancel 可长期保持 Running，违反注释声明的取消清理 grace。

## 证据

`echo-orchestration/src/tasks/command_cell.rs` 的 run result、finalization、terminal publication 与 tests 提供源码反例和覆盖缺口。

## 处理记录

Time-lifecycle Audit 确认。`finish_output_artifact` 现监听 cell/owner cancellation，普通 stop 与 owner cancellation 的 blocking finalizer 测试通过；repair、verification 与独立复审已闭合本 Finding。跨平台进程组、loom/stress 和真实慢文件系统保留为验证限制。
