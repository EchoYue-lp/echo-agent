---
schema_version: 1
id: finding.transcript-generation-runtime-identity
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: data_durability
focus: [state_authority, failure_concurrency, contract_evidence]
boundary_ref: boundary.context-memory
behavior_refs: [behavior.context-memory-lifecycle]
rule_refs: [rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation]
audit_refs: [audit.context-memory.data-durability]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Runtime state 与 transcript generation identity 可写出不可恢复 checkpoint

## 问题

Invocation context 允许 `runtime_state_id` 与 `transcript_generation_id` 独立设置且不校验；保存分别使用两者，恢复却要求 checkpoint 的 runtime scope 与 cursor generation 相等。

## 触发条件与影响

调用方提供不同 identity 时可成功写入 checkpoint，随后恢复因 identity mismatch 失败，形成持久但不可消费的状态。

## 证据

`src/agent/snapshot.rs` 展示保存 identity；`src/state/mod.rs` 展示恢复相等约束；现有 stream tests 只覆盖两者相同。

## 处理记录

Data-durability Audit 确认；后续 repair 应在接纳或保存前统一验证 identity，并补不同组合的重启测试。
