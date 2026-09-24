---
schema_version: 1
id: asset.extension-registries
kind: asset
title: MCP、Hook、Skill、Plugin 与 LSP Registries
asset_type: state_authority
status: needs_review
risk: high
observed_at: source:87b717676a7b51e213630677989947777b4bed441acd8c7d655fb6c96dca77ad
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

Plugin generation publication与MCP owner已在主线交付并独立复审；Host coordinator 候选已
串联 registry、publication 与 callback authority，仍待 #73 独立复审和远端交付。其它开放
Finding 各自保留验收边界。
