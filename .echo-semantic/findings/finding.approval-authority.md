---
schema_version: 1
id: finding.approval-authority
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: permission_external
focus: [state_authority, result_side_effect, contract_evidence]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.tool-permission-sandbox.permission-external]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# PermissionService 与 Shell CommandPolicy 双重 approval

## 问题

外层 PermissionService 可批准 ShellTool 的 Execute 权限，Shell 内层 CommandPolicy 仍可返回 RequiresApproval，且不会消费已有批准。

## 触发条件与影响

需要用户批准的 shell command 可能在完成一次 HumanLoop 交互后仍被拒绝，宿主必须另建 policy 才能执行。

## 证据

`src/agent/react/run/pipeline.rs` 与 `echo-tools/src/shell.rs` 展示两套不连通的 approval decision。

## 处理记录

Discovery 记录；下一阶段统一批准 receipt 或明确两层职责，避免双提示和无法执行。
