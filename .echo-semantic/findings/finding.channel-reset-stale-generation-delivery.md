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
repair_evidence_refs: [evidence.channel-generation-delivery-fence-repair]
verification_evidence_refs: [evidence.channel-generation-delivery-fence-verification]
rereview_audit_refs: [audit.channel-generation-delivery-fence-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Channel reset后旧generation回复无法fencing

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/41

## 问题

Reset立即发布新handler generation但旧stream继续；cleanup只延后callback不取消旧输出，OutboundMessage不携带incarnation。

## 触发条件与影响

用户reset后仍可收到旧会话迟到回复，delivery层无法判断generation并拒绝，新的conversation语义被旧输出污染。

## 证据

`echo-integration/src/channels/session.rs`、`channels/types.rs`与`channels/channels/mod.rs`展示reset、活动计数和无generation输出。

## 处理记录

Time Audit确认后，ADR 0057裁决reset必须cancel/fence旧generation。修复复用既有
SessionGeneration并在transport admission取得delivery permit；reset等待已接纳permit、取消旧stream，
随后才确认replacement。setup阻塞、application rotate旧permit与QQ/飞书direct send均有red/green
或定向回归，独立复审pass。最新main集成、完整门禁和PR交付完成前Finding保持open。
