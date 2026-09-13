---
schema_version: 1
id: finding.checkpoint-journal-binding
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: data_durability
focus: [state_authority, failure_concurrency, contract_evidence]
boundary_ref: boundary.observation-persistence-delivery
behavior_refs: [behavior.observation-persistence]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation]
audit_refs: [audit.observation-persistence-delivery.data-durability]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Checkpoint 未绑定来源 Journal identity

## 问题

CheckpointFrame 与文件摘要只包含 sequence/state，recover 只检查序号范围；来自 Journal B 的同序号合法 checkpoint 可被 Journal A 接受。

## 触发条件与影响

配置、文件移动或 scope mix-up 把 checkpoint 与错误 Journal 配对时，系统返回 Loaded 并暴露另一事实流的 projection；prefix prune 后更无法重建。

## 证据

`echo-state/src/journal/mod.rs`、`journal/file.rs` 与 `delivery.rs` 展示 checkpoint schema、digest 和 recover/validate 边界。

## 处理记录

Data-durability Audit 确认；后续 repair 需绑定 Journal/scope identity 并加入合法异源 checkpoint 测试。
