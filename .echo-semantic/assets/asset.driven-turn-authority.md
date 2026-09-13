---
schema_version: 1
id: asset.driven-turn-authority
kind: asset
title: Driven Turn Terminal Authority
asset_type: state_authority
status: needs_review
risk: high
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
boundary_refs: [boundary.agent-session-turn]
code_refs: [echo-orchestration/src/runtime/turn_driver.rs, echo-core/src/agent/event_envelope.rs]
consumer_refs: [src/headless.rs, src/acp/runtime.rs, tests/agent_handle_turn_driver.rs]
behavior_refs: [behavior.agent-turn-lifecycle]
rule_refs: [rule.turn-terminal-authority]
evidence_refs: [evidence.agent-context-execution]
finding_refs: [finding.turn-driver-entry-coverage]
candidate_refs: [asset.agent-execution-contract]
---

# Driven Turn Terminal Authority

## 资产身份

AgentTurnDriver/TurnReceipt 是一次 driven invocation 的 envelope sequence、terminal 与计量权威。

## 来源与消费者

Headless、ACP、经 ACP 的 SDK 与显式 driver consumers 使用；不宣称 raw Agent、Channel 或 A2A 自动经过本路径。

## 生命周期

Admit input、commit envelope、drain sink、settle Completed/Cancelled/Failed receipt；Agent 实例构造与关闭归 raw execution owner。

## 候选关系

包装 raw Agent execution，但不替代 TaskRun、SubagentAttempt、trace Run 或协议 TaskState。

## 未知与限制

Channel/direct route 尚待审计，故保持 needs_review。
