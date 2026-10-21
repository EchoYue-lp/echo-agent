---
schema_version: 1
id: finding.sdk-gap-ack-replay-watermark
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: state_authority
focus: [failure_concurrency, data_durability, contract_evidence]
boundary_ref: boundary.protocol-surfaces
behavior_refs: [behavior.protocol-projection]
rule_refs: [rule.protocol-role-separation, rule.fact-projection-separation]
evidence_refs: [evidence.provider-protocol-quality, evidence.sdk-contracts]
audit_refs: [audit.protocol-surfaces.state-authority, audit.protocol-surfaces.time-lifecycle]
decision_refs: []
repair_evidence_refs: [evidence.provider-protocol-quality]
verification_evidence_refs: [evidence.provider-protocol-quality]
rereview_audit_refs: [audit.protocol-surfaces.state-authority]
discovered_at: 01c203b85a3451cf0c3bcfc91e68ad25534f8041
---

# SDK gap ACK 后 Host live replay 水位回退

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/120

## 问题

`StreamDelivery::acknowledge` 在 Client ACK gap `snapshot_watermark` 后仍从
`last_sent_sequence` 恢复 Journal replay。Client 已按 gap 合同把 receive cursor 推进到
snapshot watermark，Host 可能随后重发 `<= watermark` 的旧事件。

## 触发条件与影响

ACK window 或 oversized event 形成 gap，Client 接受 gap 并 ACK snapshot watermark 后，
Python、Java、TypeScript 的严格 feed 会把 Host 重发识别为倒退或不连续 sequence 并终止。
放宽 SDK 校验会掩盖 Host replay authority 错误。

## 证据

`echo-sdk-host/src/core_profile/events.rs` 的 `DeliveryInner`、
`StreamDelivery::acknowledge`、`reserve` 和 `bounded_replay` 展示 live sent、ACK-able、
snapshot 与 replay watermark 的分离。

## 处理记录

Finding #88 的 Python/Java generation 与 sequence validation 独立复审时发现。后续 repair
必须使 Host resume watermark 单调越过已 ACK 的 snapshot boundary，并用
gap -> ACK -> replay/live continuation 端到端反例证明不重发已被 snapshot 覆盖的事件。

## SDK owner 后继闭合（2026-10-20）

原类型、边界、发现版本及问题正文保留。按 ADR0051，提取后的 Host 实现由独立
SDK维护；这里结算同一历史缺陷，不在 framework 复制实现或接受风险。

SDK精main `09f670fbbde98bc44a7c710c6419a6ea0d7d469c` 的[同名Finding](https://github.com/EchoYue-lp/echo-agent-sdk/blob/09f670fbbde98bc44a7c710c6419a6ea0d7d469c/.echo-semantic/findings/finding.sdk-gap-ack-replay-watermark.md)
已具备修复、验证和独立复审三链。ACK snapshot watermark后恢复cursor单调推进，
三语言严格sequence校验未放宽。本仓 evidence.provider-protocol-quality 的两个角色段
路由原反例、各语言12个场景及精main交付，state-authority Audit的新challenge仅审owner路由。

[Issue120闭合评论](https://github.com/EchoYue-lp/echo-agent/issues/120#issuecomment-6289991562)
及协调者读回记录closed/completed于2026-10-20T11:38:41Z。Issue状态不替代修复证据。
仅结算原watermark问题；四个A2A、旧审计/历史计数、managed未知、广域hold、native与全goal不变。
