---
schema_version: 1
id: asset.extension-registries
kind: asset
title: MCP、Hook、Skill、Plugin 与 LSP Registries
asset_type: state_authority
status: needs_review
risk: high
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
boundary_refs: [boundary.extension-lifecycle]
code_refs: [echo-integration/src/mcp/mod.rs, echo-execution/src/skills/hooks.rs, echo-execution/src/skills/registry.rs, echo-core/src/plugin/registry.rs, src/plugin/prepared.rs, echo-core/src/plugin/lifecycle.rs, echo-integration/src/lsp/manager.rs]
consumer_refs: [src/agent/react/capabilities.rs, echo-sdk-host/src/core_profile/facade/integrations.rs]
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
finding_refs: [finding.skill-activation-authority, finding.hook-permission-precedence, finding.mcp-client-capability-advertisement, finding.plugin-mcp-owner-isolation, finding.plugin-failure-isolation-contract, finding.plugin-lifecycle-coordination, finding.lsp-runtime-state, finding.extension-cleanup-settlement, finding.mcp-version-doc-drift]
candidate_refs: []
---

# MCP、Hook、Skill、Plugin 与 LSP Registries

## 资产身份

五类 extension 的发现、命名、activation/publication、owner/generation 与撤销状态集合。

## 来源与消费者

ReactAgent capabilities、PluginIntegrator、SDK Host 和 public APIs 消费。

## 生命周期

Discover/connect/prepare/apply/activate/start，reload/replace，unwire/deactivate/stop/close。

## 候选关系

内部包含多个独立 authority；本资产只表示跨 extension 边界，不合并状态。

## 未知与限制

双 Skill state、Plugin lifecycle、MCP owner 和 LSP recovery 已形成 Findings。
