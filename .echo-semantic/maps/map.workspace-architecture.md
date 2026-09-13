---
schema_version: 1
id: map.workspace-architecture
kind: capability_map
title: Workspace 架构与公共组合
risk: high
observed_at: source:5a12b544f08f549cccd424c6c0a22acf3f1cba0e15bcacc1febc283b76536f6b
boundary_refs: [boundary.workspace-architecture]
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure, evidence.high-risk-audit-frontier]
finding_refs: [finding.workspace-topology-doc-drift, finding.public-feature-table-drift, finding.readme-example-target-drift]
audit_refs: [audit.workspace-architecture.contract-evidence]
related_map_refs: [map.agent-session-turn, map.context-memory, map.task-subagent-workflow, map.observation-persistence-delivery, map.tool-permission-sandbox, map.extension-lifecycle, map.llm-provider-runtime, map.protocol-surfaces, map.eval-evolution]
scenarios:
  crate-dag-and-layering:
    status: needs_review
    source_refs: [Cargo.toml, src/lib.rs, echo-core/src/lib.rs]
    behavior_refs: [behavior.workspace-composition]
    rule_refs: [rule.framework-layer-ownership]
    evidence_refs: [evidence.workspace-structure]
    finding_refs: [finding.workspace-topology-doc-drift]
    unknown: Cargo拓扑已闭合，但双语README遗漏SDK crates并报告错误package数量
    next_step: 修复双语拓扑并让文档合同从Cargo metadata校验成员集合
  feature-topology:
    status: needs_review
    source_refs: [Cargo.toml, echo-core/Cargo.toml, echo-execution/Cargo.toml, echo-integration/Cargo.toml, echo-macros/Cargo.toml, echo-orchestration/Cargo.toml, echo-state/Cargo.toml, echo-tools/Cargo.toml, echo-sdk-protocol/Cargo.toml, echo-sdk-host/Cargo.toml, echo-agent-learning/Cargo.toml]
    rule_refs: [rule.framework-layer-ownership]
    evidence_refs: [evidence.workspace-structure]
    finding_refs: [finding.public-feature-table-drift]
    unknown: Manifest feature权威已映射，但双语README公开不存在的tasks feature
    next_step: 修复双语feature表并增加manifest-derived contract check
  public-and-background-entrypoints:
    status: needs_review
    source_refs: [src/lib.rs, echo-sdk-host/src/main.rs, echo-orchestration/src/scheduler/runner.rs, echo-orchestration/src/tasks/background_task.rs]
    behavior_refs: [behavior.workspace-composition]
    evidence_refs: [evidence.workspace-structure]
    finding_refs: [finding.readme-example-target-drift]
    unknown: README把test contract写成不存在的Cargo example target
    next_step: 修正命令并让root README target进入documentation contract
  dynamic-registration-inventory:
    status: needs_review
    source_refs: [echo-macros/src/lib.rs, echo-execution/src/skills/registry.rs, echo-core/src/plugin/registry.rs, src/workflow/loader.rs]
    unknown: procedural macro、Plugin/Skill/Hook、MCP config 和 declarative workflow 的全部 runtime consumers 尚未形成单一静态闭集
    next_step: 在 extension 与 protocol 高风险 audit 中验证每个注册入口的 owner、generation 和卸载路径
  product-workspace-device:
    status: excluded
    source_refs: [docs/en/39-framework-application-boundary.md]
    reason: EKO Workspace identity、Device sync、UI/TUI/CLI projection 是应用策略，不属于通用 framework 状态权威
    risk: high
    recheck_when: 出现不依赖 EKO product types 且可独立编译测试的通用能力提案时
---

# Workspace 架构与公共组合

## 能力范围

覆盖 11-package Cargo workspace、crate DAG、feature topology、root facade、binary/background entry 与 framework/application 边界。

## 入口与输出

输入是 Cargo feature、public API、builder/config 与 Host CLI；输出是编译后的 capability surface 和具体子边界实例。

## 行为关系

Root facade 组合 split crates，SDK Host/learning/tests 消费公共 surface；具体运行语义由相关 Capability Map 拥有。

## 状态与数据流

Manifest 是编译能力权威，root facade 是公共路径权威；不保存运行态或产品 Workspace 数据。

## 策略来源与优先级

Cargo manifests、AGENTS.md、ADR 0013/0014 和公开 docs 依次约束构建、分层与消费者。

## 生命周期与失败路径

Feature 缺失、依赖环、public path drift 和 example/contract drift 由 compile/test/CI 暴露；后台资源由子 map 结算。

## 权限与敏感信息

本 map 只路由权限/外部 effect，不新增产品权限门禁；credential 由 provider/extension 子边界处理。

## 用户侧投影

Rust facade、SDK Host、docs 与 examples 展示同一 capability；采用量不决定 public API 是否合理。

## 场景处置清单

全部 11 个 package manifest、crate、entry 与 product boundary 已映射；动态注册闭集显式保留 needs_review。

## 未展开项

各运行时状态机、持久化、协议和 effect 由 related maps 展开。
