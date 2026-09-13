---
schema_version: 1
id: asset.workspace-manifest
kind: asset
title: Workspace 与 Feature 编译权威
asset_type: state_authority
status: active
risk: high
observed_at: source:1edd0f8dd43db91c544af47174e3f57154b9598d3bd78d9a3e7859a422f24a91
boundary_refs: [boundary.workspace-architecture]
code_refs: [Cargo.toml, echo-core/Cargo.toml, echo-execution/Cargo.toml, echo-integration/Cargo.toml, echo-macros/Cargo.toml, echo-orchestration/Cargo.toml, echo-state/Cargo.toml, echo-tools/Cargo.toml, echo-sdk-protocol/Cargo.toml, echo-sdk-host/Cargo.toml, echo-agent-learning/Cargo.toml]
consumer_refs: [.github/workflows/rust-ci.yml, scripts/verify.sh, README.md, README.zh.md, echo-agent-learning/tests/documentation_contract.rs]
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure, evidence.workspace-topology-doc-repair, evidence.workspace-topology-doc-verification]
finding_refs: []
candidate_refs: []
---

# Workspace 与 Feature 编译权威

## 资产身份

Cargo workspace members、依赖 DAG 与 feature 声明的编译期权威。

## 来源与消费者

全部 11 个 package manifests 被 Cargo、CI、learning passthrough 和 SDK Host feature advertisement 消费。

## 生命周期

Manifest 变化在编译和 contract generation 时生效。

## 候选关系

没有平行 workspace authority；各 consumer 只能转发或检查。

## 未知与限制

运行时配置不由本资产决定。
