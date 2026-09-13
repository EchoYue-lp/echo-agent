---
schema_version: 1
id: asset.framework-docs
kind: asset
title: Framework 正式文档与 ADR
asset_type: document
status: needs_review
risk: medium
observed_at: source:c205eb521ef63e2d37d921693a1a0703b253144b575642baa3ec94c3ba2d75b3
boundary_refs: [boundary.workspace-architecture]
code_refs: [README.md, README.zh.md, docs/en/README.md, docs/zh/README.md, docs/en/24-eval-system.md, docs/zh/24-eval-system.md, docs/adr/0014-framework-capability-placement.md, docs/adr/0033-subagent-factory-singleflight-publication.md, docs/adr/0034-context-scoped-tool-result-cache.md, docs/adr/0035-owned-tool-registry-handles.md, docs/adr/0036-eval-workspace-generation-lifecycle.md]
consumer_refs: [echo-agent-learning/tests/documentation_contract.rs]
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure]
finding_refs: []
candidate_refs: []
---

# Framework 正式文档与 ADR

## 资产身份

公共概念、API、架构决策、examples 路由与长期维护说明。

## 来源与消费者

Framework 用户、SDK consumers、website 同步和 documentation contracts 消费。

## 生命周期

架构/API 变化时与代码、examples 和 tests 同步更新。

## 候选关系

顶层跨仓计划和历史审计不是本资产的长期行为 authority。

## 未知与限制

Workspace 图、Task feature、example path、MCP version 和 Evolution namespace 已形成文档 Finding。
