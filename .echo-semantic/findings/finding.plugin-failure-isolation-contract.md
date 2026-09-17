---
schema_version: 1
id: finding.plugin-failure-isolation-contract
kind: finding
type: intent_gap
status: resolved
severity: medium
primary_focus: contract_evidence
focus: [failure_concurrency, state_authority]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions, evidence.plugin-component-preparation-repair, evidence.plugin-component-preparation-verification]
audit_refs: [audit.extension-lifecycle.state-authority, audit.plugin-component-preparation-rereview]
decision_refs: []
repair_evidence_refs: [evidence.plugin-component-preparation-repair]
verification_evidence_refs: [evidence.plugin-component-preparation-verification]
rereview_audit_refs: [audit.plugin-component-preparation-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Plugin component isolation 与 atomic generation 冲突

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/71

## 问题

正式文档称坏 Skill/组件按最小边界隔离，SkillLoader error 却使完整 PreparedPluginSet generation 不可应用；ADR 0012 支持后者。

## 触发条件与影响

一个 Plugin 内单个无效 Skill 时，用户无法从文档判断其它 Hook/MCP/Subagent 是否仍应发布。

## 证据

`docs/en/32-plugin-system.md`、`echo-execution/src/skills/external/loader.rs`、`src/plugin/prepared.rs` 与 ADR 0012 表达不同合同。

## 处理记录

ADR 0045 DU-71已确认“prepare阶段组件隔离、完整immutable generation原子发布”。
`PluginIntegrator::prepare`现仅让依赖排序、完整Plugin准备或generation分配等代次级失败阻断
apply；无效Skill/Hook/MCP组件被排除并保留error diagnostic，健康兄弟组件可继续发布。
EKO第二阶段同样隔离Subagent/LSP/product component并投影诊断。Focused验证与独立复审已
闭合本Finding；#72/#73/#74/#75继续独立追踪发布、生命周期与owner结算。
