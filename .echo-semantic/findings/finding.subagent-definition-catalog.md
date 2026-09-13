---
schema_version: 1
id: finding.subagent-definition-catalog
kind: finding
type: intent_gap
status: open
severity: medium
primary_focus: contract_evidence
focus: [trigger_input, state_authority]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Definition-only Subagent catalog 合同冲突

## 问题

SubagentRegistry 文档声称 definition-only 项进入 available/catalog，生产实现与测试却过滤没有 instance/factory 的定义。

## 触发条件与影响

Plugin 或配置只注册 definition 时，调用方可能看到与文档不同的可调度列表，影响模型选择和错误分类。

## 证据

`src/agent/subagent/registry.rs` 的注释、过滤实现和相反测试构成直接合同冲突。

## 处理记录

Discovery 记录；下一阶段裁决 advertised-but-not-runnable 或 hidden-until-resolvable，再统一文档和测试。
