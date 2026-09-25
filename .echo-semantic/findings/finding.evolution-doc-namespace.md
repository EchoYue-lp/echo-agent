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
evidence_refs: [evidence.provider-protocol-quality, evidence.evolution-doc-namespace-repair, evidence.evolution-doc-namespace-verification]
audit_refs: [audit.eval-evolution.data-durability]
decision_refs: []
repair_evidence_refs: [evidence.evolution-doc-namespace-repair]
verification_evidence_refs: [evidence.evolution-doc-namespace-verification]
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Evolution 文档 namespace 与代码漂移

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/53

## 问题

代码使用 `agent/memories` warm namespace 并默认折叠 cold 层，双语文档仍描述 `typed_memories` 和旧三层模型。

## 触发条件与影响

Framework consumer 按文档查询或设计持久布局时，会使用代码不再读取的 namespace/层次。

## 证据

`src/evolution/layer.rs` 与 `docs/en/25-self-improvement.md`、对应中文文档直接冲突。

## 处理记录

已按 `MemoryLayerManager` 当前读写路径修正双语文档、交叉引用及
documentation contract；独立复审和 main 交付前保持 open。
