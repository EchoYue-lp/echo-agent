---
schema_version: 1
id: finding.a2a-terminal-authority
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [time_lifecycle, failure_concurrency, contract_evidence]
boundary_ref: boundary.protocol-surfaces
behavior_refs: [behavior.protocol-projection, behavior.agent-turn-lifecycle]
rule_refs: [rule.protocol-role-separation, rule.turn-terminal-authority]
evidence_refs: [evidence.provider-protocol-quality, evidence.agent-context-execution]
audit_refs: [audit.protocol-surfaces.state-authority, audit.protocol-surfaces.contract-evidence]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# A2A server 自行拥有第二套执行终态

## 问题

A2AServer 维护 TaskState/task map/cancel map，直接消费 Agent stream 并自行判定 completed/failed/canceled；stream completion 还可覆盖已写入 Canceled，未复用 AgentTurnDriver/TurnReceipt。

## 触发条件与影响

A2A 调用遇到 EOF、sink error、cancel 或 usage/terminal 边界时，可能与 ACP/Headless/SDK 的统一 Turn 事实不同。

## 证据

`src/a2a/server.rs` 与 `src/a2a/types.rs` 展示独立 task/terminal reducer。

## 处理记录

Discovery 记录；A2A 固定 wire TaskState 可保留，但内部执行应审计为 TurnReceipt projection。
