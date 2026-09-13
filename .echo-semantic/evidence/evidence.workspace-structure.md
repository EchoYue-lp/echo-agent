---
schema_version: 1
id: evidence.workspace-structure
kind: evidence
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
source_refs:
  - Cargo.toml
  - echo-core/Cargo.toml
  - echo-execution/Cargo.toml
  - echo-integration/Cargo.toml
  - echo-macros/Cargo.toml
  - echo-orchestration/Cargo.toml
  - echo-state/Cargo.toml
  - echo-tools/Cargo.toml
  - echo-sdk-protocol/Cargo.toml
  - echo-sdk-host/Cargo.toml
  - echo-agent-learning/Cargo.toml
  - src/lib.rs
  - echo-core/src/lib.rs
  - echo-execution/src/lib.rs
  - echo-integration/src/lib.rs
  - echo-orchestration/src/lib.rs
  - echo-state/src/lib.rs
  - echo-tools/src/lib.rs
  - echo-sdk-protocol/src/lib.rs
  - echo-sdk-host/src/lib.rs
  - echo-agent-learning/src/lib.rs
  - tests/facade_smoke.rs
  - echo-agent-learning/tests/documentation_contract.rs
  - docs/adr/0013-learning-examples-and-documentation-boundary.md
  - scripts/verify.sh
  - .github/workflows/rust-ci.yml
supports: [behavior.workspace-composition, rule.framework-layer-ownership]
limitations:
  - 证明 workspace 结构、公共组合入口和消费者存在，不证明各运行时边界内部行为正确
---

# Workspace 结构证据

## 支持的结论

根 package 与十个成员构成 11-package workspace；`echo_core` 提供基础合同，execution/state/orchestration 提供机制，integration/tools 提供实现，root `echo_agent` 组合公共 facade，SDK Host 与 learning package 是消费者。

## 来源与范围

来源包括全部 11 个 package manifests、各 crate 根模块、root facade、公共 facade smoke、learning 文档合同以及本地/CI 门禁入口。

## 已知缺口

动态注册、feature 组合的完整运行语义和每个后台任务的资源清理由其它边界建模；本 Evidence 不把当前采用量当作 framework API 存废依据。
