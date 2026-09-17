---
schema_version: 1
id: finding.transcript-generation-runtime-identity
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: data_durability
focus: [state_authority, failure_concurrency, contract_evidence]
boundary_ref: boundary.context-memory
behavior_refs: [behavior.context-memory-lifecycle]
rule_refs: [rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation, evidence.transcript-generation-runtime-identity-repair, evidence.transcript-generation-runtime-identity-verification]
audit_refs: [audit.context-memory.data-durability, audit.transcript-generation-runtime-identity-rereview]
decision_refs: []
repair_evidence_refs: [evidence.transcript-generation-runtime-identity-repair]
verification_evidence_refs: [evidence.transcript-generation-runtime-identity-verification]
rereview_audit_refs: [audit.transcript-generation-runtime-identity-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Runtime state 与 transcript generation identity 可写出不可恢复 checkpoint

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/105

## 问题

Invocation context 允许 `runtime_state_id` 与 `transcript_generation_id` 独立设置且不校验；保存分别使用两者，恢复却要求 checkpoint 的 runtime scope 与 cursor generation 相等。

## 触发条件与影响

调用方提供不同 identity 时可成功写入 checkpoint，随后恢复因 identity mismatch 失败，形成持久但不可消费的状态。

## 证据

`src/agent/snapshot.rs` 展示保存 identity；`src/state/mod.rs` 展示恢复相等约束；现有 stream tests 只覆盖两者相同。

## 处理记录

修复复用唯一 effective runtime identity resolver，并在 stream admission 与 checkpoint save 边界
双重 fail closed。A/B mismatch 在 mutex、guard、trace、input drain、context、LLM 与 Store 副作用前
被拒绝；相等、product fallback、None 兼容和既有 corrupt checkpoint 恢复拒绝均通过回归。
完整本地门禁与两轮独立复审通过，本 Finding 已闭合。
