---
schema_version: 1
id: asset.agent-execution-contract
kind: asset
title: Raw Agent Execution Contract
asset_type: protocol
status: active
risk: high
observed_at: source:0ff44ba1010dfd579acdd80c3f9d369c3d87f1dcbe05ba8840f5de7c96be4e61
boundary_refs: [boundary.agent-session-turn]
code_refs: [echo-core/src/agent/mod.rs, src/agent/react/mod.rs, src/agent/handle.rs]
consumer_refs: [src/channels.rs, src/a2a/server.rs, echo-orchestration/src/runtime/turn_driver.rs]
behavior_refs: [behavior.agent-turn-lifecycle]
rule_refs: []
evidence_refs: [evidence.agent-context-execution, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification]
finding_refs: [finding.turn-driver-entry-coverage, finding.a2a-terminal-authority]
candidate_refs: [asset.driven-turn-authority]
---

# Raw Agent Execution Contract

## 资产身份

Agent trait、ReactAgent 与 AgentHandle 提供 execute/chat/stream/cancel/close 的原始 framework execution contract，不自动生成 TurnReceipt。

## 来源与消费者

Channel、A2A与直接Rust callers可调用raw execution；Eval已迁移为AgentTurnDriver consumer，各adapter必须自行声明是否需要driven Turn terminal。

## 生命周期

Construct/configure Agent、execute/chat/stream、cancel、close；ReactAgent managed stream terminal不领先于其自有producer settlement，具体Invocation resource和effect由相关运行边界结算。

## 候选关系

可由 AgentTurnDriver 包装为 driven invocation，但 raw contract 与 driver terminal authority 不是同一状态权威。

## 未知与限制

Channel/A2A 是否必须采用 driver 已形成 Findings；不能据此删除合理的低层 public Agent API。
