---
schema_version: 1
id: asset.journal-delivery
kind: asset
title: EventJournal、Checkpoint 与 DeliveryLedger
asset_type: state_authority
status: active
risk: high
observed_at: 44b2ed68772c7c016d09af6c2e1adac9fe4fea70
boundary_refs: [boundary.observation-persistence-delivery]
code_refs: [echo-state/src/journal/mod.rs, echo-state/src/journal/file.rs, echo-state/src/journal/segmented.rs, echo-state/src/delivery.rs, docs/adr/0055-checkpoint-journal-identity.md]
consumer_refs: [src/state/mod.rs]
behavior_refs: [behavior.observation-persistence]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation, evidence.checkpoint-journal-binding-repair, evidence.checkpoint-journal-binding-verification, evidence.checkpoint-journal-sdk-inventory]
finding_refs: [finding.checkpoint-journal-binding]
candidate_refs: []
---

# EventJournal、Checkpoint 与 DeliveryLedger

## 资产身份

Ordered event commit/replay、checkpointed reduction 与 typed delivery lifecycle 的持久 authority。

## 来源与消费者

Delivery、SDK persistence 和 framework consumers 使用 generic journal/checkpoint APIs。

## 生命周期

Allocate generation identity、prepare/append batch、reconcile unknown outcome、reduce/bound checkpoint/recover、claim/effect/ack/drain/settle/prune。

## 候选关系

Trace 和 transcript 不替代 journal fact。

## 未知与限制

每个应用 payload/retention policy 仍由消费者拥有。

## 跨仓生产消费者承接

ADR0051提取后的SDK生产消费者由独立SDK拥有；原F观察、状态、全部业务限制和既有正文保持。consumer_refs仅列原F消费者，不把外仓路径误作本F revision中的文件。

| 原F定位与角色 | 实际SDK承接 | 不可变源码 | Git blob |
|---|---|---|---|
| `echo-sdk-host/src/core_profile/persistence.rs`，F观察 `44b2ed68772c7c016d09af6c2e1adac9fe4fea70`，production_source | SDK `map.sdk-events-recovery`；boundary.sdk-events-recovery; behavior.sdk-events-recovery | [SDK生产源码](https://github.com/EchoYue-lp/echo-agent-sdk/blob/7f14a1b9c751b0612ceb42224f83f29d09aea46c/echo-sdk-host/src/core_profile/persistence.rs)；[既有Map](https://github.com/EchoYue-lp/echo-agent-sdk/blob/7f14a1b9c751b0612ceb42224f83f29d09aea46c/.echo-semantic/maps/map.sdk-events-recovery.md) | `18d3e76ec57882a301d177edbea38b4362ace2e8` |

上述生产文件在既有Map观察、公开SDK `7f14a1b9c751b0612ceb42224f83f29d09aea46c`、SDK `019e700279677a2eb77e265b5038d9d581bc22da`及纠正时工作树均为相同字节。该表保留原生产角色和外仓Owner，属于跨仓归属与原件定位说明；当前单仓合同没有跨仓强引用解析器，不宣称正文链接构成新增机器强边，也不宣称519项连续性已通过。
