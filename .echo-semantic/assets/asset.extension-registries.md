---
schema_version: 1
id: asset.extension-registries
kind: asset
title: MCP、Hook、Skill、Plugin 与 LSP Registries
asset_type: state_authority
status: needs_review
risk: high
observed_at: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
boundary_refs: [boundary.extension-lifecycle]
code_refs: [echo-integration/src/mcp/mod.rs, echo-execution/src/skills/hooks.rs, echo-execution/src/skills/registry.rs, echo-core/src/plugin/registry.rs, src/plugin/coordinator.rs, src/plugin/prepared.rs, src/agent/react/mod.rs, echo-core/src/plugin/lifecycle.rs, echo-integration/src/lsp/manager.rs]
consumer_refs: [src/agent/react/capabilities.rs, echo-sdk-host/src/core_profile/facade/integrations.rs]
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions, evidence.plugin-generation-publication-authority-repair, evidence.plugin-generation-publication-authority-verification, evidence.plugin-lifecycle-coordinator-repair, evidence.plugin-lifecycle-coordinator-verification]
finding_refs: [finding.skill-activation-authority, finding.hook-permission-precedence, finding.mcp-client-capability-advertisement, finding.plugin-mcp-owner-isolation, finding.plugin-failure-isolation-contract, finding.plugin-lifecycle-coordination, finding.lsp-runtime-state, finding.extension-cleanup-settlement, finding.mcp-version-doc-drift]
candidate_refs: []
---

# MCP、Hook、Skill、Plugin 与 LSP Registries

## 资产身份

五类 extension 的发现、命名、activation/publication、owner/generation 与撤销状态集合。

## 来源与消费者

ReactAgent capabilities、PluginIntegrator、SDK Host 和 public APIs 消费；每个ReactAgent独立持有Plugin active publication与cleanup receipt。

## 生命周期

Discover/connect/prepare/apply/activate/start，reload/replace，unwire/deactivate/stop/close。

## 候选关系

内部包含多个独立 authority；本资产只表示跨 extension 边界，不合并状态。

## 未知与限制

Plugin generation publication、MCP owner 与 Host coordinator 已在主线交付并独立复审。
MCP construction cleanup owner 在当前 #55 工作树通过 focused 验证与独立复审，待完整
门禁、PR/CI 和远端 main 交付；其它开放 Finding 各自保留验收边界。
