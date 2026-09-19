---
schema_version: 1
id: map.extension-lifecycle
kind: capability_map
title: MCP、Hook、Skill、Plugin 与 LSP 生命周期
risk: high
observed_at: source:3a8aba9cee4bdf94c17e2039bb9bc22ebcf6fb112bfe26ce5686002569409849
boundary_refs: [boundary.extension-lifecycle]
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority, rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.high-risk-audit-frontier, evidence.framework-concept-navigation, evidence.extension-credential-debug-redaction-repair, evidence.extension-credential-debug-redaction-verification, evidence.skill-activation-authority-repair, evidence.skill-activation-authority-verification, evidence.mcp-tool-local-classification-repair, evidence.mcp-tool-local-classification-verification, evidence.mcp-client-capability-advertisement-repair, evidence.mcp-client-capability-advertisement-verification, evidence.mcp-protocol-negotiation-repair, evidence.mcp-protocol-negotiation-verification, evidence.lsp-derived-handle-lifecycle-repair, evidence.lsp-derived-handle-lifecycle-verification, evidence.plugin-component-preparation-repair, evidence.plugin-generation-publication-authority-repair, evidence.plugin-generation-publication-authority-verification, evidence.plugin-mcp-owner-isolation-repair, evidence.plugin-mcp-owner-isolation-verification]
finding_refs: [finding.skill-activation-authority, finding.hook-permission-precedence, finding.hook-protected-path, finding.hook-event-producer-contract, finding.mcp-client-capability-advertisement, finding.mcp-tool-permission-classification, finding.plugin-mcp-owner-isolation, finding.plugin-failure-isolation-contract, finding.plugin-lifecycle-coordination, finding.plugin-generation-publication-authority, finding.plugin-lifecycle-reconcile-overlap, finding.lsp-runtime-state, finding.lsp-manager-derived-handle-resurrection, finding.extension-cleanup-settlement, finding.extension-credential-debug-redaction, finding.mcp-version-doc-drift]
audit_refs: [audit.extension-lifecycle.state-authority, audit.extension-lifecycle.time-lifecycle, audit.extension-lifecycle.permission-external, audit.extension-lifecycle.contract-evidence, audit.skill-activation-authority-rereview, audit.mcp-tool-local-classification-rereview, audit.mcp-client-capability-advertisement-rereview, audit.mcp-protocol-negotiation-rereview, audit.lsp-derived-handle-lifecycle-rereview, audit.plugin-component-preparation-rereview, audit.plugin-generation-publication-authority-rereview, audit.plugin-mcp-owner-isolation-rereview]
related_map_refs: [map.workspace-architecture, map.context-memory, map.tool-permission-sandbox, map.protocol-surfaces]
scenarios:
  mcp-connect-discover-close:
    status: mapped
    source_refs: [echo-integration/src/mcp/client.rs, echo-integration/src/mcp/mod.rs, echo-integration/src/mcp/transport/sse.rs]
    finding_refs: [finding.mcp-client-capability-advertisement, finding.mcp-tool-permission-classification, finding.extension-cleanup-settlement, finding.extension-credential-debug-redaction, finding.mcp-version-doc-drift]
    evidence_refs: [evidence.effects-extensions, evidence.mcp-tool-local-classification-repair, evidence.mcp-tool-local-classification-verification, evidence.mcp-client-capability-advertisement-repair, evidence.mcp-client-capability-advertisement-verification, evidence.mcp-protocol-negotiation-repair, evidence.mcp-protocol-negotiation-verification]
    audit_refs: [audit.mcp-tool-local-classification-rereview, audit.mcp-client-capability-advertisement-rereview, audit.mcp-protocol-negotiation-rereview]
  hook-source-and-reduction:
    status: mapped
    source_refs: [echo-core/src/hooks/types.rs, echo-execution/src/skills/hooks.rs]
    finding_refs: [finding.hook-permission-precedence, finding.hook-protected-path]
    rule_refs: [rule.permission-effect-order]
  skill-discovery-activation-restore:
    status: mapped
    source_refs: [echo-execution/src/skills/external/loader.rs, echo-execution/src/skills/registry.rs, src/agent/react/capabilities.rs, src/agent/react/mod.rs]
    finding_refs: [finding.skill-activation-authority]
    evidence_refs: [evidence.effects-extensions, evidence.skill-activation-authority-repair, evidence.skill-activation-authority-verification]
  plugin-prepare-publish-withdraw:
    status: mapped
    source_refs: [echo-core/src/plugin/registry.rs, echo-core/src/plugin/lifecycle.rs, echo-integration/src/mcp/identity.rs, echo-integration/src/mcp/mod.rs, echo-integration/src/mcp/resource_tool.rs, src/plugin/prepared.rs, src/agent/react/capabilities.rs, src/agent/react/mod.rs]
    finding_refs: [finding.plugin-mcp-owner-isolation, finding.plugin-failure-isolation-contract, finding.plugin-lifecycle-coordination, finding.plugin-generation-publication-authority, finding.plugin-lifecycle-reconcile-overlap]
    rule_refs: [rule.extension-generation-authority]
    evidence_refs: [evidence.plugin-component-preparation-repair, evidence.plugin-generation-publication-authority-repair, evidence.plugin-generation-publication-authority-verification, evidence.plugin-mcp-owner-isolation-repair, evidence.plugin-mcp-owner-isolation-verification]
    audit_refs: [audit.plugin-generation-publication-authority-rereview, audit.plugin-mcp-owner-isolation-rereview]
  lsp-process-routing:
    status: mapped
    source_refs: [echo-core/src/lsp/client.rs, echo-integration/src/lsp/client.rs, echo-integration/src/lsp/manager.rs]
    finding_refs: [finding.lsp-runtime-state, finding.lsp-manager-derived-handle-resurrection, finding.extension-cleanup-settlement]
    evidence_refs: [evidence.lsp-derived-handle-lifecycle-repair, evidence.lsp-derived-handle-lifecycle-verification]
    audit_refs: [audit.lsp-derived-handle-lifecycle-rereview]
  host-production-coordination:
    status: needs_review
    source_refs: [src/plugin/prepared.rs, echo-core/src/plugin/lifecycle.rs]
    unknown: Plugin registry/wiring/callback 与 LSP/MCP close 的统一生产编排者在 framework consumers 中不完整
    next_step: high-risk lifecycle audit 追踪 Host 与 embedding application 的实际调用顺序和 cleanup debt
  credential-debug-redaction:
    status: mapped
    source_refs: [echo-integration/src/redaction.rs, echo-integration/src/mcp/config_loader.rs, echo-integration/src/mcp/server_config.rs, echo-integration/src/mcp/transport/mod.rs, echo-integration/src/mcp/transport/http.rs, echo-integration/src/mcp/transport/sse.rs, echo-integration/src/mcp/transport/stdio.rs, echo-integration/src/channels/channels/qq/channel.rs, echo-integration/src/channels/channels/qq/api.rs, echo-integration/src/channels/channels/qq/gateway.rs, echo-integration/src/channels/channels/feishu/channel.rs, echo-integration/src/channels/channels/feishu/api.rs, echo-integration/src/channels/channels/feishu/long_poll.rs, echo-integration/src/channels/channels/feishu/webhook.rs]
    finding_refs: [finding.extension-credential-debug-redaction]
    evidence_refs: [evidence.extension-credential-debug-redaction-repair, evidence.extension-credential-debug-redaction-verification]
---

# MCP、Hook、Skill、Plugin 与 LSP 生命周期

## 能力范围

覆盖五类 extension 的发现、解析、命名、注册、激活、发布、热更新、撤销、关闭、generation 与 rollback。

## 入口与输出

用户/project/plugin config、SDK operation 与 Agent runtime 触发；输出 Tool/Resource/Hook/Skill/Subagent/LSP projections 和 child process/network effect。

## 行为关系

每类 registry 管自身状态，Plugin prepared generation 组合发布；跨组件不得因统一 Plugin 包而丢失 owner。

## 状态与数据流

MCP client/topology、Hook sources/result、Plugin registry/prepared/lifecycle、LSP config/client maps 分别演进。Plugin active publication和receipt的唯一authority在每个ReactAgent，Integrator只共享prepare cache。Skill descriptor/prepared document保留定义视图，主registry与progressive tool adapter共享唯一activation state。

## 策略来源与优先级

Scope/owner/config、official Skill/Plugin/MCP contracts、Hook source order 和 feature capabilities 决定发布。

## 生命周期与失败路径

Discover/prepare/connect/apply/activate/start，replace/reload，unwire/deactivate/stop/close；partial failure 需 rollback 或 cleanup debt。

## 权限与敏感信息

MCP annotation、Hook permission、Skill allowlist/sandbox 与 Plugin variables 不得泄漏 credential 或绕过不可替代的本地数据保护。

## 用户侧投影

Catalog/status/tool list 是 registry projection；仅可执行且当前 generation 的组件才能被广告。

## 场景处置清单

五类生命周期和十六个Finding已映射；Skill activation authority、Plugin prepare failure
isolation、active generation 与 MCP owner isolation 已在主线修复并通过独立复审。跨 Host
统一编排仍由 #73 保持 needs_review。

## 未展开项

EKO Plugin fan-out、preferences 和 UI catalog 属应用边界。
