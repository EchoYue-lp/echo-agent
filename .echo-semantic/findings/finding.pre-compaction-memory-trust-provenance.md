---
schema_version: 1
id: finding.pre-compaction-memory-trust-provenance
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: permission_external
focus: [data_durability, trigger_input, state_authority]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution, behavior.context-memory-lifecycle]
rule_refs: [rule.quality-observation-boundary, rule.context-persistence-separation]
evidence_refs: [evidence.provider-protocol-quality, evidence.agent-context-execution]
audit_refs: [audit.eval-evolution.permission-external]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Pre-compaction memory丢失混合来源trust provenance

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/76

## 问题

Pre-compaction flush从user/assistant/tool混合transcript经LLM生成内容，无exact-evidence/origin验证，统一标L3Promotion+Active写入warm memory；Recall只排除Superseded。

## 触发条件与影响

工具输出或assistant推断可被提升为跨轮次可召回Active memory，来源可信度和批准语义丢失并影响后续prompt。

## 证据

`src/agent/react/run/phases/compact.rs`、`run/context.rs`与`src/evolution/recall.rs`展示自动写入和召回路径。

## 处理记录

Permission Audit确认；后续repair保留evidence span/source role/trust并默认proposal或Draft，除非满足明确promotion policy。
