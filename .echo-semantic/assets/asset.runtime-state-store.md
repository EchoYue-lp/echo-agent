---
schema_version: 1
id: asset.runtime-state-store
kind: asset
title: RuntimeStateStore 与 AgentCheckpoint
asset_type: state_authority
status: active
risk: high
observed_at: source:bfa5b4590c617d8286f2b80571d5c450622d47c7425d6f1ecb1978bf85743352
boundary_refs: [boundary.context-memory, boundary.observation-persistence-delivery]
code_refs: [src/state/mod.rs, src/state/file.rs, src/state/sqlite.rs, src/agent/snapshot.rs]
consumer_refs: [src/agent/react/run/context.rs, src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/tools.rs, src/agent/react/run/phases/finalize.rs, docs/en/03-memory.md]
behavior_refs: [behavior.context-memory-lifecycle, behavior.observation-persistence]
rule_refs: [rule.context-persistence-separation, rule.fact-projection-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation, evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification]
finding_refs: [finding.transcript-projection-settlement]
candidate_refs: []
---

# RuntimeStateStore 与 AgentCheckpoint

## 资产身份

ReAct runtime checkpoint、revisioned pending transcript intent、scope lineage、generation tombstone 与
managed clear/delete retirement manifest 的持久权威。

## 来源与消费者

ReactAgent hydration/finalization 消费，file 与 optional SQLite 提供 backend。

## 生命周期

Load/hydrate、pending prepare/retry/proof-ack、rotate/clear incarnation、scope retirement 与 stable
conversation delete saga。Attempt result 与 dispatch 归属由 CAS revision 约束。

## 候选关系

不替代 ConversationStore、Task graph 或 Trace。

## 未知与限制

`current_plan` 的生产写入来源仍需审查。
