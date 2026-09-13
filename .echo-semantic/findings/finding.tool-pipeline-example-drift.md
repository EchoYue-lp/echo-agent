---
schema_version: 1
id: finding.tool-pipeline-example-drift
kind: finding
type: evidence_gap
status: open
severity: medium
primary_focus: contract_evidence
focus: [time_lifecycle, result_side_effect]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution, behavior.workspace-composition]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.workspace-structure]
audit_refs: [audit.observation-persistence-delivery.contract-evidence, audit.tool-permission-sandbox.result-side-effect]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# demo64 Tool pipeline 合同与生产顺序漂移

## 问题

Executable contract 仍声明 13 stages、包含已不存在的 ParseValidateStage，并把 Trace 放在 PostHook 前；生产 pipeline 当前是 16 stages 且顺序不同。

## 触发条件与影响

文档、学习示例或审查以 demo64 为事实源时，会得到错误的权限、观察和副作用顺序；测试只打印静态数组，无法检测生产漂移。

## 证据

`echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs` 与 `src/agent/react/run/pipeline.rs` 构成反例。

## 处理记录

Discovery 记录；后续 repair 应让 contract 直接消费 production metadata 或用结构测试绑定真实顺序。
