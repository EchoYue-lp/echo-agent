---
schema_version: 1
id: asset.root-facade
kind: asset
title: Root echo_agent 公共 Facade
asset_type: entrypoint
status: active
risk: high
observed_at: f1e9027246760661144786e9e35615cd46d580c6
boundary_refs: [boundary.workspace-architecture]
code_refs: [src/lib.rs]
consumer_refs: [tests/facade_smoke.rs, echo-agent-learning/src/lib.rs, echo-sdk-host/src/lib.rs]
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure]
finding_refs: []
candidate_refs: []
---

# Root echo_agent 公共 Facade

## 资产身份

Downstream Rust 使用者的 canonical capability path、prelude 与 advanced entry。

## 来源与消费者

由 split crate implementation 组合，被 examples、tests 和 SDK Host 消费。

## 生命周期

随 crate feature 编译；不存在独立运行态。

## 候选关系

Split-crate paths 是实现来源，不是第二 public facade。

## 未知与限制

Public path 存在不代表每个 downstream 都已采用。
