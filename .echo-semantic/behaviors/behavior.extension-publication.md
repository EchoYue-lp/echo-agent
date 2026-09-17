---
schema_version: 1
id: behavior.extension-publication
kind: behavior
status: needs_review
expectation: inferred
risk: high
primary_focus: failure_concurrency
focus: [state_authority, time_lifecycle, result_side_effect, permission_external, data_durability]
boundary: boundary.extension-lifecycle
observed_at: source:bfa5b4590c617d8286f2b80571d5c450622d47c7425d6f1ecb1978bf85743352
code_refs: [echo-integration/src/mcp/mod.rs, echo-execution/src/skills/hooks.rs, echo-execution/src/skills/registry.rs, echo-core/src/plugin/registry.rs, src/plugin/prepared.rs, echo-integration/src/lsp/manager.rs]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions, evidence.skill-activation-authority-repair, evidence.skill-activation-authority-verification, evidence.mcp-tool-local-classification-repair, evidence.mcp-tool-local-classification-verification, evidence.mcp-client-capability-advertisement-repair, evidence.mcp-client-capability-advertisement-verification, evidence.lsp-derived-handle-lifecycle-repair, evidence.lsp-derived-handle-lifecycle-verification, evidence.plugin-component-preparation-repair, evidence.plugin-component-preparation-verification]
finding_refs: [finding.skill-activation-authority, finding.hook-permission-precedence, finding.mcp-client-capability-advertisement, finding.mcp-tool-permission-classification, finding.plugin-mcp-owner-isolation, finding.plugin-failure-isolation-contract, finding.plugin-lifecycle-coordination, finding.lsp-runtime-state, finding.lsp-manager-derived-handle-resurrection, finding.extension-cleanup-settlement, finding.extension-credential-debug-redaction, finding.mcp-version-doc-drift]
---

# MCP、Hook、Skill、Plugin 与 LSP 发布

## 重要承诺

每类扩展只有一个发现/注册 authority；prepare/publish/withdraw/close 使用 owner 与 generation，失败时保留 rollback 或 cleanup debt。

## 当前行为

McpManager管命名连接且client只广告已实现capability；HookRegistry管source/order/action reduction；Skill definition view可为异步tool适配而复制，但主registry、tool adapter、run snapshot与checkpoint共享epoch-fenced activation handle。PluginRegistry/Integrator/Lifecycle管持久状态、不可变generation与callback。LspManager唯一拥有client child process，派生handle共享其generation/closed fence。

## 期望行为

同名、reload、部分失败、断连和撤销不得让一个 owner 删除另一个 owner 的资源。Plugin
prepare在组件边界排除无效输入并保留诊断，健康兄弟组件仍进入同一个不可变generation；
代次级不变量失败时完整set拒绝发布。

## 触发、结果与副作用

用户/项目/plugin 配置与 SDK 操作触发发现和发布；Hook command/http/MCP/Subagent、Skill script、MCP/LSP child process 产生外部 effect。

## 失败、重试与恢复

Prepare/apply/rollback、transport close、pending call、LSP EOF/restart、Plugin disable/uninstall 和 checkpoint restore 必须有一致的结算路径。

## 证据

各 registry/manager、immutable prepared generation tests、Hook order tests、MCP close tests 和 ADR 0012/0023/0026 提供基础证据。

## 裁决记录

Skill activation、Hook permission precedence、MCP capability advertisement/local
classification、LSP派生handle与ADR 0045 DU-71确认的Plugin component preparation isolation
均已修复并独立复审。Plugin active generation、lifecycle coordination、reconcile overlap、
MCP owner isolation、LSP runtime status与异步cleanup仍为open Finding，因此本Behavior继续
保持needs_review。
