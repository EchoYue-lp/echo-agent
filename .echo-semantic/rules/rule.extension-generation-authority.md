---
schema_version: 1
id: rule.extension-generation-authority
kind: rule
status: needs_review
expectation: inferred
risk: high
primary_focus: failure_concurrency
focus: [state_authority, time_lifecycle, data_durability, permission_external]
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
behavior_refs: [behavior.extension-publication]
code_refs: [echo-integration/src/mcp/mod.rs, echo-execution/src/skills/hooks.rs, echo-execution/src/skills/registry.rs, echo-core/src/plugin/registry.rs, src/plugin/prepared.rs, echo-core/src/plugin/lifecycle.rs, echo-integration/src/lsp/manager.rs]
evidence_refs: [evidence.effects-extensions]
finding_refs: [finding.skill-activation-authority, finding.hook-permission-precedence, finding.mcp-client-capability-advertisement, finding.plugin-mcp-owner-isolation, finding.plugin-failure-isolation-contract, finding.plugin-lifecycle-coordination, finding.lsp-runtime-state, finding.extension-cleanup-settlement, finding.mcp-version-doc-drift]
---

# Extension Owner 与 Generation

## 不变量或唯一权威

MCP、Hook、Skill、Plugin 和 LSP 各自使用明确 owner/registry；Plugin publish 基于不可变 prepared generation 和可回滚 wiring receipt。

## 适用行为

适用于 discovery、install/enable/disable/uninstall、prepare/apply/unwire、reload、connect/disconnect、start/stop 和 checkpoint restore。

## 当前实现

各 manager/registry 持有部分状态；PluginRegistry、PluginIntegrator 与 PluginLifecycleManager 分别负责持久状态、wiring generation 和 callbacks。

## 期望行为

同名资源不能跨 owner 被错误替换或撤销；部分 apply/close 失败保留可恢复 debt；文档 failure isolation 与实际 generation atomicity 一致。

## 证据

MCP/Hook/Skill/Plugin/LSP 源码、ADR 0012/0023/0026 和 focused tests 提供部分证据。

## 裁决记录

双 Skill state、Plugin/MCP owner、Plugin lifecycle 与 LSP recovery 已进入 open Finding，故本 Rule 尚待审计。
