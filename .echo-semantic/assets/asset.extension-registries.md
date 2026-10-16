---
schema_version: 1
id: asset.extension-registries
kind: asset
title: MCP、Hook、Skill、Plugin 与 LSP Registries
asset_type: state_authority
status: needs_review
risk: high
observed_at: 733d352fc719f922b21bab1cd46206139564367f
boundary_refs: [boundary.extension-lifecycle]
code_refs: [echo-integration/src/mcp/mod.rs, echo-execution/src/skills/hooks.rs, echo-execution/src/skills/registry.rs, echo-core/src/plugin/registry.rs, src/plugin/coordinator.rs, src/plugin/prepared.rs, src/agent/react/mod.rs, echo-core/src/plugin/lifecycle.rs, echo-integration/src/lsp/manager.rs]
consumer_refs: [src/agent/react/capabilities.rs]
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

## 跨仓生产消费者承接

ADR0051提取后的SDK生产消费者由独立SDK拥有；原F观察、状态、全部业务限制和既有正文保持。consumer_refs仅列原F消费者，不把外仓路径误作本F revision中的文件。

| 原F定位与角色 | 实际SDK承接 | 不可变源码 | Git blob |
|---|---|---|---|
| `echo-sdk-host/src/core_profile/facade/integrations.rs`，F观察 `733d352fc719f922b21bab1cd46206139564367f`，production_source | SDK `map.sdk-integrations`；boundary.sdk-integrations; behavior.sdk-family-mcp / behavior.sdk-family-lsp | [SDK生产源码](https://github.com/EchoYue-lp/echo-agent-sdk/blob/7f14a1b9c751b0612ceb42224f83f29d09aea46c/echo-sdk-host/src/core_profile/facade/integrations.rs)；[既有Map](https://github.com/EchoYue-lp/echo-agent-sdk/blob/7f14a1b9c751b0612ceb42224f83f29d09aea46c/.echo-semantic/maps/map.sdk-integrations.md) | `b38cae06325cffc20b3d9d883070f957d5d20cf8` |

上述生产文件在既有Map观察、公开SDK `7f14a1b9c751b0612ceb42224f83f29d09aea46c`、SDK `019e700279677a2eb77e265b5038d9d581bc22da`及纠正时工作树均为相同字节。该表保留原生产角色和外仓Owner，属于跨仓归属与原件定位说明；当前单仓合同没有跨仓强引用解析器，不宣称正文链接构成新增机器强边，也不宣称519项连续性已通过。
