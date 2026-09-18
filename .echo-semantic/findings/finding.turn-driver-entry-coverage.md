---
schema_version: 1
id: finding.turn-driver-entry-coverage
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [time_lifecycle, failure_concurrency, contract_evidence]
boundary_ref: boundary.agent-session-turn
behavior_refs: [behavior.agent-turn-lifecycle, behavior.protocol-projection]
rule_refs: [rule.turn-terminal-authority, rule.protocol-role-separation]
evidence_refs: [evidence.agent-context-execution, evidence.provider-protocol-quality]
audit_refs: [audit.agent-session-turn.state-authority, audit.protocol-surfaces.state-authority, audit.protocol-surfaces.contract-evidence, audit.channel-driven-turn-rereview]
decision_refs: []
repair_evidence_refs: [evidence.channel-driven-turn-repair]
verification_evidence_refs: [evidence.channel-driven-turn-verification]
rereview_audit_refs: [audit.channel-driven-turn-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Channel adapter 绕过 driven Turn authority

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/107

## 问题

Headless、ACP 与经 ACP 的 SDK 使用 `AgentTurnDriver`/`TurnReceipt`，但 Channel handler 作为外部 adapter 只调用 `ReactAgent::chat`，没有投影 driven Turn identity/receipt。Raw execute/chat 是合理低层 public API，本身不构成缺陷。

## 触发条件与影响

Channel invocation 遇到 EOF、sink failure、cancel、usage accounting 或 close 时，没有共享 TurnReceipt 作为 adapter terminal 事实。

## 证据

`src/headless.rs`、`src/acp/runtime.rs` 展示 driven path；`src/channels.rs` 展示 Channel bypass，raw API 合同由 `echo-core/src/agent/mod.rs` 限定。

## 处理记录

状态权威 Audit 已收窄问题；后续 repair/decision 只处理承诺有限 Turn 生命周期的 Channel adapter，不强迫低层 Agent API生成 receipt。

框架 `AgentChannelHandler` 已改用 `AgentTurnDriver` 并公开单次调用的 `TurnReceipt`；
标准回复仅接受 `Completed + Delivered + final_answer`。EKO 应用 channel 自有 handler
通过 `drive_foreground_pooled_chat_turn` 进入同一个框架 driver，本 Finding 不修改其产品
投影。Session reset 现在向 driven handler 传播 generation cancel，并等待 active driven
setup 释放 receipt 后才确认替换。当前保留 open：`ChannelManager::stop_all` 的
QQ/飞书 adapter close 结算属于 Finding #36，待该依赖、独立复审、完整门禁和远端 main
交付均闭合后再关闭。
