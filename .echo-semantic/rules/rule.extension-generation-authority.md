---
schema_version: 1
id: rule.extension-generation-authority
kind: rule
status: needs_review
expectation: inferred
risk: high
primary_focus: failure_concurrency
focus: [state_authority, time_lifecycle, data_durability, permission_external]
observed_at: source:5806bd920140828b759fabe868fc1c4af1f26009ed4bf9112b73385cf2ce764e
behavior_refs: [behavior.extension-publication]
code_refs: [echo-integration/src/mcp/mod.rs, echo-execution/src/skills/hooks.rs, echo-execution/src/skills/registry.rs, echo-core/src/plugin/registry.rs, src/plugin/prepared.rs, src/agent/react/mod.rs, echo-core/src/plugin/lifecycle.rs, echo-integration/src/lsp/manager.rs]
evidence_refs: [evidence.effects-extensions, evidence.skill-activation-authority-repair, evidence.skill-activation-authority-verification, evidence.mcp-client-capability-advertisement-repair, evidence.mcp-client-capability-advertisement-verification, evidence.lsp-derived-handle-lifecycle-repair, evidence.lsp-derived-handle-lifecycle-verification, evidence.plugin-component-preparation-repair, evidence.plugin-component-preparation-verification, evidence.plugin-generation-publication-authority-repair, evidence.plugin-generation-publication-authority-verification]
finding_refs: [finding.skill-activation-authority, finding.hook-permission-precedence, finding.mcp-client-capability-advertisement, finding.mcp-tool-permission-classification, finding.plugin-mcp-owner-isolation, finding.plugin-failure-isolation-contract, finding.plugin-lifecycle-coordination, finding.lsp-runtime-state, finding.lsp-manager-derived-handle-resurrection, finding.extension-cleanup-settlement, finding.mcp-version-doc-drift]
---

# Extension Owner 与 Generation

## 不变量或唯一权威

MCP、Hook、Skill、Plugin 和 LSP 各自使用明确 owner/registry；Plugin publish 基于不可变 prepared generation 和可回滚 wiring receipt。

## 适用行为

适用于 discovery、install/enable/disable/uninstall、prepare/apply/unwire、reload、connect/disconnect、start/stop 和 checkpoint restore。

## 当前实现

各manager/registry持有自身状态；SkillRegistry主视图、definition adapter、run snapshot与checkpoint共享唯一epoch/generation-fenced activation handle。MCP client只协商已实现capability；LspManager的generation/closed lifecycle覆盖所有派生client。PluginRegistry、PluginIntegrator与PluginLifecycleManager分别负责持久状态、immutable preparation和callbacks；prepare的进程级序号跨独立Integrator排序，ReactAgent持有唯一target-scoped active publication与cleanup receipt，cloned Integrator不拥有全局active generation。MCP每次成功连接在下一次await前登记receipt，原本缺席的名字在连接期间保留清理scope。

## 期望行为

同名资源不能跨 owner 被错误替换或撤销；部分 apply/close 失败保留可恢复 debt。Plugin
prepare的组件错误只排除该组件并记录诊断，健康兄弟保持在同一不可变generation；依赖排序
等代次级失败仍阻断完整set。

## 证据

MCP/Hook/Skill/Plugin/LSP 源码、ADR 0012/0023/0026 和 focused tests 提供部分证据。

## 裁决记录

Skill state、MCP capability/local classification与LSP derived ownership已修复复审；ADR 0045
DU-71与ADR 0012也已统一component isolation和atomic generation合同。Plugin active generation
已在主线 `cb4ee9ed` 交付target-scoped authority并通过独立复审；lifecycle coordination、MCP
owner isolation及其它开放Finding不由本切片关闭，故本Rule继续待审计。
