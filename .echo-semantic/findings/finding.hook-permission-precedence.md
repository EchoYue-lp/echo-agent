---
schema_version: 1
id: finding.hook-permission-precedence
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: permission_external
focus: [state_authority, failure_concurrency, contract_evidence]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication, behavior.effect-permission-execution]
rule_refs: [rule.extension-generation-authority, rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.tool-permission-sandbox.permission-external, audit.extension-lifecycle.permission-external]
decision_refs: []
repair_evidence_refs: [evidence.hook-permission-precedence-repair]
verification_evidence_refs: [evidence.hook-permission-precedence-verification]
rereview_audit_refs: []
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

产品裁决确认 Agent 自动 Tool 权限采用全局 deny-wins。`HookAction::Permission` 已移除
隐式 propagation stop，并补 UserConfig allow、Plugin ask、Skill deny 的跨来源组合测试；
独立复审和主线语义门禁仍待完成。
