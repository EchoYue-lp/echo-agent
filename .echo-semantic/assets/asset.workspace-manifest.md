---
schema_version: 1
id: asset.workspace-manifest
kind: asset
title: Workspace 与 Feature 编译权威
asset_type: state_authority
status: active
risk: high
observed_at: source:469a276a3666fa7b9f836bc4c5751516ca360fcdb16efae6cc81c2d66ffb2560
boundary_refs: [boundary.workspace-architecture]
code_refs: [Cargo.toml, echo-core/Cargo.toml, echo-execution/Cargo.toml, echo-integration/Cargo.toml, echo-macros/Cargo.toml, echo-orchestration/Cargo.toml, echo-state/Cargo.toml, echo-tools/Cargo.toml, echo-agent-learning/Cargo.toml]
consumer_refs: [.github/workflows/rust-ci.yml, scripts/verify.sh, README.md, README.zh.md, echo-agent-learning/tests/documentation_contract.rs]
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure, evidence.workspace-topology-doc-repair, evidence.workspace-topology-doc-verification, evidence.feature-table-doc-repair, evidence.feature-table-doc-verification, evidence.readme-example-target-repair, evidence.readme-example-target-verification, evidence.framework-concept-navigation]
finding_refs: []
candidate_refs: []
---

# Workspace 与 Feature 编译权威

## 资产身份

Cargo workspace members、依赖 DAG 与 feature 声明的编译期权威。

## 来源与消费者

9 个 framework/runtime package manifests 被 Cargo、CI、learning passthrough 和外部 SDK
consumer 消费；SDK Host 自身的 manifest 位于独立 `echo-agent-sdk` 仓库。

## 生命周期

Manifest 变化在编译和 contract generation 时生效。

## 候选关系

没有平行 workspace authority；各 consumer 只能转发或检查。

## 未知与限制

运行时配置不由本资产决定。
