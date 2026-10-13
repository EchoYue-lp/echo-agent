---
schema_version: 1
id: asset.workspace-manifest
kind: asset
title: Workspace 与 Feature 编译权威
asset_type: state_authority
status: active
risk: high
observed_at: source:1c515c09873968a279e6076cf6abf659923f1fa96137144423970bfcd0e1881c
boundary_refs:
- boundary.workspace-architecture
code_refs:
- Cargo.toml
- echo-core/Cargo.toml
- echo-execution/Cargo.toml
- echo-integration/Cargo.toml
- echo-macros/Cargo.toml
- echo-orchestration/Cargo.toml
- echo-state/Cargo.toml
- echo-tools/Cargo.toml
- echo-agent-learning/Cargo.toml
- Cargo.lock
- docs/dependencyrepair.md
- docs/evidence/dependency-repair-20261013/receipt.json
consumer_refs:
- .github/workflows/rust-ci.yml
- scripts/verify.sh
- README.md
- README.zh.md
- echo-agent-learning/tests/documentation_contract.rs
behavior_refs:
- behavior.workspace-composition
rule_refs:
- rule.framework-layer-ownership
evidence_refs:
- evidence.workspace-structure
- evidence.workspace-topology-doc-repair
- evidence.workspace-topology-doc-verification
- evidence.feature-table-doc-repair
- evidence.feature-table-doc-verification
- evidence.readme-example-target-repair
- evidence.readme-example-target-verification
- evidence.framework-concept-navigation
- evidence.framework-dependency-policy-repair
- evidence.framework-dependency-policy-focused-verification
- evidence.framework-dependency-final-full-verification
- evidence.framework-dependency-final-gate-archive
finding_refs:
- finding.framework-dependency-policy-ci
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


本轮依赖边界：六Cargo文件的maintained YAML alias与quinn锁替换属于框架编译实现，复用原公共API与调用路径，原audit/deny门禁权威保持；focused/policyredgreen不代新全量与交付验收。旧已verified分层规则未改为其它risk/依赖全部通过。


依赖本地修复收口：新23完整工程最终通过及独立源码review PASS/action0见 evidence.framework-dependency-final-full-verification / audit.framework-dependency-policy-rereview，原分层/状态不扩展为remote/consumer/全risk已验。
