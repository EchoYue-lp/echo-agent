---
schema_version: 1
id: finding.transcript-projection-settlement
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: data_durability
focus: [time_lifecycle, failure_concurrency, contract_evidence]
boundary_ref: boundary.context-memory
behavior_refs: [behavior.context-memory-lifecycle]
rule_refs: [rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation, evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification, evidence.framework-only-finding-closure-verification]
audit_refs: [audit.context-memory.data-durability, audit.transcript-projection-settlement-rereview]
decision_refs: [decision-adr-0056-durable-transcript-projection-settlement]
repair_evidence_refs: [evidence.transcript-projection-settlement-repair]
verification_evidence_refs: [evidence.transcript-projection-settlement-verification, evidence.framework-only-finding-closure-verification]
rereview_audit_refs: [audit.transcript-projection-settlement-rereview]
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

Framework 现以 `RuntimeStateStore` revision/CAS 持有唯一 pending intent，以
`ConversationStore` receipt 持有已提交 transcript fact；prepare/apply/ack、warm/cold recovery、
compact、tool、guard、hook、terminal、exact clear 与 managed delete 统一经过 store-backed
coordinator。只配置 ConversationStore 会在 admission 副作用前拒绝；timeout 保留 typed durable debt，
Blocked/Conflict 抑制原业务终态。

File/SQLite parity、deadline authority lock、attempt/retry、lost-ack、scope retirement、recreate、
direct/stream 终态与独立复审均已通过。`44b2ed68` 的 durable coordinator 与
`3735f7e0` 的 observer ordering 都已进入 framework main；独立 consumer 的 protocol、Host
与语言映射由其所属仓库验证，不是本 Finding 或 Issue #106 的关闭条件。
