---
schema_version: 1
id: finding.channel-attachment-projection
kind: finding
type: intent_gap
status: resolved
severity: medium
primary_focus: result_side_effect
focus: [contract_evidence, trigger_input]
boundary_ref: boundary.protocol-surfaces
behavior_refs: [behavior.protocol-projection]
rule_refs: [rule.protocol-role-separation]
evidence_refs: [evidence.provider-protocol-quality, evidence.channel-attachment-projection-repair, evidence.channel-attachment-projection-verification]
audit_refs: [audit.protocol-surfaces.state-authority, audit.protocol-surfaces.contract-evidence]
decision_refs: []
repair_evidence_refs: [evidence.channel-attachment-projection-repair]
verification_evidence_refs: [evidence.channel-attachment-projection-verification]
rereview_audit_refs: [audit.channel-attachment-projection-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Channel attachment 在 Agent adapter 中丢失

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/40

## 问题

InboundMessage 保存 attachments，AgentChannelHandler 只调用 `agent.chat(&msg.text)`，没有传递多模态内容；当前内建 QQ/飞书声明不支持 media，custom/future media channel 才会即时触发。

## 触发条件与影响

QQ/飞书等 channel 收到附件时，Agent 只看到文本，TUI/GUI/channel 功能对等目标可能被破坏。

## 证据

`echo-integration/src/channels/types.rs` 与 `src/channels.rs` 的 adapter 调用提供静态证据。

## 处理记录

复用 `Message::user_multimodal` 和 `TurnRequest::from_message`，有附件时按顺序投影
为 typed user message；无附件仍走原文本入口。图像 MIME 按字节签名识别，文件字节
以 base64 保留，不可表示的媒体在模型调用前拒绝。独立复审和本地完整合并门禁通过；
Issue 仍须等远端主线交付及 CI 核对后关闭。
