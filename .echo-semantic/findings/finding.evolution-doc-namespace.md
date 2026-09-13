---
schema_version: 1
id: finding.evolution-doc-namespace
kind: finding
type: evidence_gap
status: open
severity: medium
primary_focus: contract_evidence
focus: [data_durability]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.eval-evolution.data-durability]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Evolution 文档 namespace 与代码漂移

## 问题

代码使用 `agent/memories` warm namespace 并默认折叠 cold 层，双语文档仍描述 `typed_memories` 和旧三层模型。

## 触发条件与影响

Framework consumer 按文档查询或设计持久布局时，会使用代码不再读取的 namespace/层次。

## 证据

`src/evolution/layer.rs` 与 `docs/en/25-self-improvement.md`、对应中文文档直接冲突。

## 处理记录

Discovery 记录；下一阶段先确认当前持久 contract，再同步双语文档和 examples。
