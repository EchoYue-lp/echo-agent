---
schema_version: 1
id: map.extension-lifecycle
kind: capability_map
title: MCP、Hook、Skill、Plugin 与 LSP 生命周期
risk: high
observed_at: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
boundary_refs: [boundary.extension-lifecycle]
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority, rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.high-risk-audit-frontier, evidence.framework-concept-navigation, evidence.extension-credential-debug-redaction-repair, evidence.extension-credential-debug-redaction-verification, evidence.skill-activation-authority-repair, evidence.skill-activation-authority-verification, evidence.mcp-tool-local-classification-repair, evidence.mcp-tool-local-classification-verification, evidence.mcp-client-capability-advertisement-repair, evidence.mcp-client-capability-advertisement-verification, evidence.mcp-protocol-negotiation-repair, evidence.mcp-protocol-negotiation-verification, evidence.lsp-derived-handle-lifecycle-repair, evidence.lsp-derived-handle-lifecycle-verification, evidence.plugin-component-preparation-repair, evidence.plugin-generation-publication-authority-repair, evidence.plugin-generation-publication-authority-verification, evidence.plugin-mcp-owner-isolation-repair, evidence.plugin-mcp-owner-isolation-verification, evidence.plugin-lifecycle-coordinator-repair, evidence.plugin-lifecycle-coordinator-verification]
finding_refs: [finding.skill-activation-authority, finding.hook-permission-precedence, finding.hook-protected-path, finding.hook-event-producer-contract, finding.mcp-client-capability-advertisement, finding.mcp-tool-permission-classification, finding.plugin-mcp-owner-isolation, finding.plugin-failure-isolation-contract, finding.plugin-lifecycle-coordination, finding.plugin-generation-publication-authority, finding.plugin-lifecycle-reconcile-overlap, finding.lsp-runtime-state, finding.lsp-manager-derived-handle-resurrection, finding.extension-cleanup-settlement, finding.extension-credential-debug-redaction, finding.mcp-version-doc-drift]
audit_refs: [audit.extension-lifecycle.state-authority, audit.extension-lifecycle.time-lifecycle, audit.extension-lifecycle.permission-external, audit.extension-lifecycle.contract-evidence, audit.skill-activation-authority-rereview, audit.mcp-tool-local-classification-rereview, audit.mcp-client-capability-advertisement-rereview, audit.mcp-protocol-negotiation-rereview, audit.lsp-derived-handle-lifecycle-rereview, audit.plugin-component-preparation-rereview, audit.plugin-generation-publication-authority-rereview, audit.plugin-mcp-owner-isolation-rereview, audit.plugin-lifecycle-coordinator-rereview]
related_map_refs: [map.workspace-architecture, map.context-memory, map.tool-permission-sandbox, map.protocol-surfaces]
scenarios:
  mcp-connect-discover-close:
    status: needs_review
    source_refs: [echo-integration/src/mcp/client.rs, echo-integration/src/mcp/mod.rs, echo-integration/src/mcp/transport/sse.rs]
    finding_refs: [finding.mcp-client-capability-advertisement, finding.mcp-tool-permission-classification, finding.extension-cleanup-settlement, finding.extension-credential-debug-redaction, finding.mcp-version-doc-drift]
    evidence_refs: [evidence.effects-extensions, evidence.mcp-tool-local-classification-repair, evidence.mcp-tool-local-classification-verification, evidence.mcp-client-capability-advertisement-repair, evidence.mcp-client-capability-advertisement-verification, evidence.mcp-protocol-negotiation-repair, evidence.mcp-protocol-negotiation-verification, evidence.extension-cleanup-settlement-repair, evidence.extension-cleanup-settlement-verification]
    audit_refs: [audit.mcp-tool-local-classification-rereview, audit.mcp-client-capability-advertisement-rereview, audit.mcp-protocol-negotiation-rereview, audit.extension-cleanup-settlement-rereview]
    unknown: MCP cleanup在当前任务分支由retained scope等待并可重试；本地完整门禁与17项独立feature已通过，PR/CI和远端main仍待交付，MCP版本文档与其它独立Finding另行验收
    next_step: 完成#55的PR/CI与远端main交付，并单独处理MCP版本文档合同
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
    source_refs: [echo-core/src/plugin/registry.rs, echo-core/src/plugin/lifecycle.rs, echo-integration/src/mcp/identity.rs, echo-integration/src/mcp/mod.rs, echo-integration/src/mcp/resource_tool.rs, src/plugin/coordinator.rs, src/plugin/prepared.rs, src/agent/react/capabilities.rs, src/agent/react/mod.rs]
    finding_refs: [finding.plugin-mcp-owner-isolation, finding.plugin-failure-isolation-contract, finding.plugin-lifecycle-coordination, finding.plugin-generation-publication-authority, finding.plugin-lifecycle-reconcile-overlap]
    rule_refs: [rule.extension-generation-authority]
    evidence_refs: [evidence.plugin-component-preparation-repair, evidence.plugin-generation-publication-authority-repair, evidence.plugin-generation-publication-authority-verification, evidence.plugin-mcp-owner-isolation-repair, evidence.plugin-mcp-owner-isolation-verification, evidence.plugin-lifecycle-coordinator-repair, evidence.plugin-lifecycle-coordinator-verification]
    audit_refs: [audit.plugin-generation-publication-authority-rereview, audit.plugin-mcp-owner-isolation-rereview]
  lsp-process-routing:
    status: mapped
    source_refs: [echo-core/src/lsp/client.rs, echo-integration/src/lsp/client.rs, echo-integration/src/lsp/manager.rs]
    finding_refs: [finding.lsp-runtime-state, finding.lsp-manager-derived-handle-resurrection]
    evidence_refs: [evidence.lsp-derived-handle-lifecycle-repair, evidence.lsp-derived-handle-lifecycle-verification]
    audit_refs: [audit.lsp-derived-handle-lifecycle-rereview]
  host-production-coordination:
    status: mapped
    source_refs: [src/plugin/coordinator.rs, src/plugin/prepared.rs, echo-core/src/plugin/registry.rs, echo-core/src/plugin/lifecycle.rs]
    finding_refs: [finding.plugin-lifecycle-coordination, finding.hook-event-producer-contract]
    evidence_refs: [evidence.plugin-lifecycle-coordinator-repair, evidence.plugin-lifecycle-coordinator-verification]
    audit_refs: [audit.plugin-lifecycle-coordinator-rereview]
    unknown: framework Host统一编排已通过完整门禁、三轮独立复审与remote-main delivery，并在verified commit dc61ef0e闭合；#58 durable Hook producer acknowledgement仍未闭合
    next_step: "#58 继续单独验收跨进程 Hook delivery；不再把#73路由为待交付候选"
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
统一编排已覆盖 framework Host 主路径与失败重试矩阵，并以verified `dc61ef0e`进入远端main；
#73已完成，#58继续覆盖跨进程 Hook producer acknowledgement。MCP construction cleanup
owner 在本任务分支通过完整本地门禁、17项独立feature检查与独立复审，待PR/CI和远端main交付。

## 未展开项

EKO Plugin fan-out、preferences 和 UI catalog 属应用边界。
