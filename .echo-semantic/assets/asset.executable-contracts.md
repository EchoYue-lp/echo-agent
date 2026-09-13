---
schema_version: 1
id: asset.executable-contracts
kind: asset
title: Tests、Examples 与 CI 消费者
asset_type: test_consumer
status: active
risk: high
observed_at: 78b9f06b4320531fd8f41260887cd69c1343e995
boundary_refs: [boundary.workspace-architecture, boundary.agent-session-turn, boundary.context-memory, boundary.task-subagent-workflow, boundary.observation-persistence-delivery, boundary.tool-permission-sandbox, boundary.extension-lifecycle, boundary.llm-provider-runtime, boundary.protocol-surfaces, boundary.eval-evolution]
code_refs: [tests/facade_smoke.rs, echo-agent-learning/tests/documentation_contract.rs, echo-agent-learning/tests/example_contracts.rs, echo-agent-learning/examples/README.md, .github/workflows/rust-ci.yml, scripts/verify.sh]
consumer_refs: [Cargo.toml, echo-agent-learning/Cargo.toml]
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure, evidence.agent-context-execution, evidence.task-subagent-workflow, evidence.persistence-observation, evidence.effects-extensions, evidence.provider-protocol-quality]
finding_refs: [finding.tool-pipeline-example-drift]
candidate_refs: []
---

# Tests、Examples 与 CI 消费者

## 资产身份

Framework public API、行为、失败路径、feature 与跨平台合同的 executable consumers。

## 来源与消费者

Cargo/CI 运行 tests/examples/benches 和 SDK checks；正式文档引用 learning contracts。

## 生命周期

随变更做 focused 验证，任务合并前执行完整门禁，CI 补平台信号。

## 候选关系

测试存在不等于当前 revision 已执行；Evidence 必须记录实际命令。

## 未知与限制

本 discovery 只盘点消费者，未把完整工程门禁写成已运行事实；demo64 与生产 pipeline 漂移已形成 Finding。
