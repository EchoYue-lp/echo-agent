---
schema_version: 1
id: finding.approval-authority
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: permission_external
focus: [state_authority, result_side_effect, contract_evidence]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.approval-authority-repair, evidence.approval-authority-verification]
audit_refs: [audit.tool-permission-sandbox.permission-external]
decision_refs: []
repair_evidence_refs: [evidence.approval-authority-repair]
verification_evidence_refs: [evidence.approval-authority-verification]
rereview_audit_refs: [audit.approval-authority-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# PermissionService 与 Shell CommandPolicy 双重 approval

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/37

## 问题

外层 PermissionService 可批准 ShellTool 的 Execute 权限，Shell 内层 CommandPolicy 仍可返回 RequiresApproval，且不会消费已有批准。

## 触发条件与影响

需要用户批准的 shell command 可能在完成一次 HumanLoop 交互后仍被拒绝，宿主必须另建 policy 才能执行。

## 证据

`src/agent/react/run/pipeline.rs` 与 `echo-tools/src/shell.rs` 展示两套不连通的 approval decision。

## 处理记录

Discovery 记录；当前候选统一为 invocation-scoped approval receipt：
PermissionService 或显式 Hook Allow 在最终 rewrite 后签发，React pipeline 通过
`ToolContext` 传入 ShellTool，Shell 的 foreground/background/streaming effect boundary
只消费与最终 tool name 和 canonical effective args 完全匹配的 receipt。完整门禁、独立复审、
远端交付与 Issue 关闭仍待验收。
