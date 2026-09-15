---
schema_version: 1
id: finding.hook-permission-precedence
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: permission_external
focus: [state_authority, failure_concurrency, contract_evidence]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication, behavior.effect-permission-execution]
rule_refs: [rule.extension-generation-authority, rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.hook-permission-precedence-repair, evidence.hook-permission-precedence-verification]
audit_refs: [audit.tool-permission-sandbox.permission-external, audit.extension-lifecycle.permission-external, audit.hook-permission-precedence-rereview]
decision_refs: []
repair_evidence_refs: [evidence.hook-permission-precedence-repair]
verification_evidence_refs: [evidence.hook-permission-precedence-verification]
rereview_audit_refs: [audit.hook-permission-precedence-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Hook source order 可绕过 deny-first 归约

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/59

## 问题

Hook reducer 声称 `deny > ask > require_approval > allow`，但每个 declarative permission action 都 stop propagation；较早 UserConfig allow 可阻止后续 Plugin/Skill deny 被看到。

## 触发条件与影响

多个 source 同时匹配 PermissionRequest 时，结果取决于 source 遍历顺序而不是文档化 deny-first 规则。

## 证据

`echo-execution/src/skills/hooks.rs` 的 source ordering、permission action 与 merge_result/short-circuit 路径构成反例。

## 处理记录

产品裁决确认 Agent 自动 Tool 权限采用全局 deny-wins。`HookAction::Permission` 不再
隐式停止传播；command/HTTP/programmatic Hook 输出即使携带 `continue: false`，只要同时
携带 permission decision，也不能隐藏后续匹配来源的 deny。跨来源与真实 command 输出反例、
focused 验证和独立复审均通过，本 Finding 已关闭。
