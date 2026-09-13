---
schema_version: 1
id: asset.driven-turn-authority
kind: asset
title: Driven Turn Terminal Authority
asset_type: state_authority
status: needs_review
risk: high
observed_at: source:0ff44ba1010dfd579acdd80c3f9d369c3d87f1dcbe05ba8840f5de7c96be4e61
boundary_refs: [boundary.agent-session-turn]
code_refs: [echo-orchestration/src/runtime/turn_driver.rs, echo-core/src/agent/event_envelope.rs]
consumer_refs: [src/headless.rs, src/acp/runtime.rs, src/eval/runner.rs, tests/agent_handle_turn_driver.rs]
behavior_refs: [behavior.agent-turn-lifecycle]
rule_refs: [rule.turn-terminal-authority]
evidence_refs: [evidence.agent-context-execution, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification]
finding_refs: [finding.turn-driver-entry-coverage, finding.eval-timeout-settlement]
candidate_refs: [asset.agent-execution-contract]
---

# Driven Turn Terminal Authority

## 资产身份

AgentTurnDriver/TurnReceipt 是一次 driven invocation 的 envelope sequence、terminal 与计量权威。

## 来源与消费者

Headless、ACP、Eval、经ACP的SDK与显式driver consumers使用；不宣称raw Agent、Channel或A2A自动经过本路径。

## 生命周期

Admit input、commit envelope、drain sink、settle Completed/Cancelled/Failed receipt；Eval可对同一drive future施加deadline与bounded settlement grace，Agent实例构造与关闭仍归raw execution owner。

## 候选关系

包装 raw Agent execution，但不替代 TaskRun、SubagentAttempt、trace Run 或协议 TaskState。

## 未知与限制

Channel/direct route 尚待审计，故保持 needs_review。
