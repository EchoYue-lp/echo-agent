---
schema_version: 1
id: finding.readonly-tools-custom-registration-bypass
kind: finding
type: implementation_bug
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

# readonly_tools 不约束 custom Write/Execute Tool

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/81

## 问题

Builder 的 readonly_tools 只让 StandardToolPack 安装只读集合，随后 custom tools 仍无条件 add；自定义 Write/Execute 工具进入所谓 read-only Agent。

## 触发条件与影响

Embedding consumer 组合 readonly_tools 与 custom mutation tool 时，模型仍可看见并执行写副作用，违背构造期只读合同。

## 证据

`src/agent/react/builder.rs` 与 `src/agent/react/mod.rs` 展示 standard/custom 注册顺序和缺少 permission-based filter。

## 处理记录

Permission Audit 确认；可与 plan-mode surface 共享 typed ToolPermission gate，但保持独立构造期测试。
