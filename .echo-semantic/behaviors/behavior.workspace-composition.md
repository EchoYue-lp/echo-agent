---
schema_version: 1
id: behavior.workspace-composition
kind: behavior
status: verified
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus:
- contract_evidence
- permission_external
- time_lifecycle
boundary: boundary.workspace-architecture
observed_at: source:1c515c09873968a279e6076cf6abf659923f1fa96137144423970bfcd0e1881c
code_refs:
- Cargo.toml
- src/lib.rs
- echo-core/src/lib.rs
- echo-execution/src/lib.rs
- echo-integration/src/lib.rs
- echo-state/src/lib.rs
- echo-orchestration/src/lib.rs
- echo-tools/src/lib.rs
- Cargo.lock
- echo-core/Cargo.toml
- echo-execution/Cargo.toml
- echo-integration/Cargo.toml
- echo-agent-learning/Cargo.toml
- docs/dependencyrepair.md
- docs/evidence/dependency-repair-20261013/receipt.json
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
---



# Workspace 组合行为

## 重要承诺

Root facade 组合 product-neutral framework 能力，split crates 保持单向依赖和独立职责，应用产品策略不反向进入 framework。

## 当前行为

`echo_core` 定义基础合同，execution/state/orchestration 提供机制，integration/tools 提供实现，root `echo_agent` 统一导出，SDK Host 和 learning package 从公共入口消费。

## 期望行为

新增能力先复用既有 crate 与 facade；跨层迁移不得形成第二状态权威或来源命名的长期 facade。

## 触发、结果与副作用

Cargo feature、builder/API、Host binary、examples 与 tests 触发不同组合；编译能力由 manifests 决定，运行副作用交给具体子边界。

## 失败、重试与恢复

缺 feature、依赖环、public path 漂移或合同不同步应在编译、facade smoke、example contract 或 CI 门禁中显式失败。

## 证据

Workspace manifests、各 crate 根模块、root facade、facade smoke、learning contracts 与 CI 入口共同提供证据。

## 裁决记录

用户确认全 workspace 治理按语义边界推进；ADR 0014 确认 framework/application 分层。


本轮依赖边界：六Cargo文件的maintained YAML alias与quinn锁替换属于框架编译实现，复用原公共API与调用路径，原audit/deny门禁权威保持；focused/policyredgreen不代新全量与交付验收。旧已verified分层规则未改为其它risk/依赖全部通过。


依赖本地修复收口：新23完整工程最终通过及独立源码review PASS/action0见 evidence.framework-dependency-final-full-verification / audit.framework-dependency-policy-rereview，原分层/状态不扩展为remote/consumer/全risk已验。
