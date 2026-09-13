---
schema_version: 1
id: finding.channel-reset-stale-generation-delivery
kind: finding
type: intent_gap
status: open
severity: high
primary_focus: time_lifecycle
focus: [failure_concurrency, result_side_effect, state_authority]
boundary_ref: boundary.protocol-surfaces
behavior_refs: [behavior.protocol-projection, behavior.agent-turn-lifecycle]
rule_refs: [rule.protocol-role-separation]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.protocol-surfaces.time-lifecycle]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Channel reset后旧generation回复无法fencing

## 问题

Reset立即发布新handler generation但旧stream继续；cleanup只延后callback不取消旧输出，OutboundMessage不携带incarnation。

## 触发条件与影响

用户reset后仍可收到旧会话迟到回复，delivery层无法判断generation并拒绝，新的conversation语义被旧输出污染。

## 证据

`echo-integration/src/channels/session.rs`、`channels/types.rs`与`channels/channels/mod.rs`展示reset、活动计数和无generation输出。

## 处理记录

Time Audit确认当前行为并要求semantic-decide：reset是允许旧输出drain，还是必须cancel/fence旧generation。
