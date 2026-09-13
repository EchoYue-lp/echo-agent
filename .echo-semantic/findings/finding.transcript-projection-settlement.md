---
schema_version: 1
id: finding.transcript-projection-settlement
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: data_durability
focus: [time_lifecycle, failure_concurrency, contract_evidence]
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

# Transcript projection 写入失败没有结算合同

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/106

## 问题

`save_transcript_projection` 在 ConversationStore ensure/save 失败时只记录 warning 并返回，未记录 retry、delivery debt 或最终缺失状态。

## 触发条件与影响

Compact/finalize safe point 遇到 backend failure 时，runtime checkpoint 可以成功而用户历史 projection 缺失，恢复和 UI history 可能观察到不一致。

## 证据

`src/agent/snapshot.rs` 是 producer；`src/agent/react/run/phases/compact.rs`、`tools.rs` 与 `finalize.rs` 展示 safe-point 调用顺序。

## 处理记录

Discovery 记录；后续 persistence audit 裁决 transcript projection 的 delivery guarantee、retry/debt 和可观察失败。
