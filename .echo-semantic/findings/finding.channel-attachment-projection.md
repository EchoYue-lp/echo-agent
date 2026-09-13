---
schema_version: 1
id: finding.channel-attachment-projection
kind: finding
type: intent_gap
status: open
severity: medium
primary_focus: result_side_effect
focus: [contract_evidence, trigger_input]
boundary_ref: boundary.protocol-surfaces
behavior_refs: [behavior.protocol-projection]
rule_refs: [rule.protocol-role-separation]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.protocol-surfaces.state-authority, audit.protocol-surfaces.contract-evidence]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Channel attachment 在 Agent adapter 中丢失

## 问题

InboundMessage 保存 attachments，AgentChannelHandler 只调用 `agent.chat(&msg.text)`，没有传递多模态内容；当前内建 QQ/飞书声明不支持 media，custom/future media channel 才会即时触发。

## 触发条件与影响

QQ/飞书等 channel 收到附件时，Agent 只看到文本，TUI/GUI/channel 功能对等目标可能被破坏。

## 证据

`echo-integration/src/channels/types.rs` 与 `src/channels.rs` 的 adapter 调用提供静态证据。

## 处理记录

Discovery 记录；下一阶段确认 channel 是否明确 text-only，若非则复用 Message multimodal contract。
