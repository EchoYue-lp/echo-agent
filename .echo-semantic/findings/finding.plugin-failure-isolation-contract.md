---
schema_version: 1
id: finding.plugin-failure-isolation-contract
kind: finding
type: intent_gap
status: open
severity: medium
primary_focus: contract_evidence
focus: [failure_concurrency, state_authority]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.extension-lifecycle.state-authority]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Plugin component isolation 与 atomic generation 冲突

## 问题

正式文档称坏 Skill/组件按最小边界隔离，SkillLoader error 却使完整 PreparedPluginSet generation 不可应用；ADR 0012 支持后者。

## 触发条件与影响

一个 Plugin 内单个无效 Skill 时，用户无法从文档判断其它 Hook/MCP/Subagent 是否仍应发布。

## 证据

`docs/en/32-plugin-system.md`、`echo-execution/src/skills/external/loader.rs`、`src/plugin/prepared.rs` 与 ADR 0012 表达不同合同。

## 处理记录

Discovery 记录；下一阶段需裁决 generation 原子性或组件隔离，再统一实现和文档。
