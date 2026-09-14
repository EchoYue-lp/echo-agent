---
schema_version: 1
id: asset.event-envelope
kind: asset
title: Versioned EventEnvelope
asset_type: protocol
status: active
risk: high
observed_at: f1e9027246760661144786e9e35615cd46d580c6
boundary_refs: [boundary.observation-persistence-delivery]
code_refs: [echo-core/src/agent/event_envelope.rs, echo-core/src/agent/mod.rs, src/agent/subagent/events.rs]
consumer_refs: [echo-orchestration/src/runtime/turn_driver.rs, src/acp/runtime.rs, echo-sdk-protocol/src/event.rs]
behavior_refs: [behavior.observation-persistence]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation]
finding_refs: []
candidate_refs: []
---

# Versioned EventEnvelope

## 资产身份

Agent/Subagent payload 的 version、identity、sequence、hash、timestamp 和 parent-link protocol。

## 来源与消费者

Turn driver、Subagent bus、ACP/SDK replay 和 observers 消费。

## 生命周期

Publisher mint、monotonic commit、bounded replay/gap、terminal reconciliation。

## 候选关系

Raw event enum 是 payload/projection，不是 envelope ordering authority。

## 未知与限制

其它 event family 的统一使用程度需 audit。
