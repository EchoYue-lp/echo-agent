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
observed_at: f1e9027246760661144786e9e35615cd46d580c6
code_refs: [echo-integration/src/mcp/mod.rs, echo-execution/src/skills/hooks.rs, echo-execution/src/skills/registry.rs, echo-core/src/plugin/registry.rs, src/plugin/prepared.rs, echo-integration/src/lsp/manager.rs]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions, evidence.skill-activation-authority-repair, evidence.skill-activation-authority-verification]
finding_refs: [finding.skill-activation-authority, finding.hook-permission-precedence, finding.mcp-client-capability-advertisement, finding.plugin-mcp-owner-isolation, finding.plugin-failure-isolation-contract, finding.plugin-lifecycle-coordination, finding.lsp-runtime-state, finding.extension-cleanup-settlement, finding.mcp-version-doc-drift]
---

# MCP、Hook、Skill、Plugin 与 LSP 发布

## 重要承诺

每类扩展只有一个发现/注册 authority；prepare/publish/withdraw/close 使用 owner 与 generation，失败时保留 rollback 或 cleanup debt。

## 当前行为

McpManager 管命名连接，HookRegistry 管 source/order/action reduction，SkillLoader/Registry 管文件与 activation；Skill definition view可为异步tool适配而复制，但Agent reconciliation同步其变更，主registry、tool adapter、run snapshot与checkpoint共享epoch-fenced activation handle。PluginRegistry/Integrator/Lifecycle 管持久状态、不可变 generation 与 callback，LspManager 管 server 路由。

## 期望行为

同名、reload、部分失败、断连和撤销不得让一个 owner 删除另一个 owner 的资源，文档与实际 failure-isolation 合同一致。

## 触发、结果与副作用

用户/项目/plugin 配置与 SDK 操作触发发现和发布；Hook command/http/MCP/Subagent、Skill script、MCP/LSP child process 产生外部 effect。

## 失败、重试与恢复

Prepare/apply/rollback、transport close、pending call、LSP EOF/restart、Plugin disable/uninstall 和 checkpoint restore 必须有一致的结算路径。

## 证据

各 registry/manager、immutable prepared generation tests、Hook order tests、MCP close tests 和 ADR 0012/0023/0026 提供基础证据。

## 裁决记录

Skill activation双权威已由canonical handle、Agent reconciliation和独立复审关闭；Hook precedence、Plugin/MCP owner、Plugin failure isolation、LSP recovery 与异步 cleanup 仍为待审 Finding。
