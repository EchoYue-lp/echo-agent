---
schema_version: 1
id: behavior.protocol-projection
kind: behavior
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: contract_evidence
focus: [state_authority, time_lifecycle, permission_external, failure_concurrency]
boundary: boundary.protocol-surfaces
observed_at: source:d7bf55a6b14c35d656f8800973a6c4b8f7642876e16cb294c5b952346c243652
code_refs: [src/acp/adapter.rs, src/acp/runtime.rs, src/a2a/server.rs, echo-integration/src/channels/manager.rs, echo-integration/src/lsp/manager.rs, src/channels.rs, src/headless.rs]
rule_refs: [rule.protocol-role-separation, rule.sdk-rust-authority]
evidence_refs: [evidence.provider-protocol-quality, evidence.sdk-contracts, evidence.lsp-derived-handle-lifecycle-repair, evidence.lsp-derived-handle-lifecycle-verification, evidence.channel-generation-delivery-fence-repair, evidence.channel-generation-delivery-fence-verification]
finding_refs: [finding.a2a-terminal-authority, finding.a2a-stream-cleanup, finding.channel-attachment-projection, finding.turn-driver-entry-coverage, finding.lsp-manager-derived-handle-resurrection]
---

# Protocol 与 Surface 投影

## 重要承诺

ACP 连接 Client 与 coding Agent，MCP 连接 Agent 与 tools/resources，A2A 连接 Agent 与 Agent；协议 adapter 应投影框架事实，不形成第二执行模型。

## 当前行为

ACP Session/Prompt/update/cancel 投影同一 driven Agent Turn；A2A server 当前自持 task/terminal；Channels 路由外部消息、以generation fence阻止reset后的旧delivery，但仍直接调用raw Agent chat；Headless 聚合 driven Turn；SDK Host 以 ACP 和 `_echo_agent/*` 暴露 Rust authority。

## 期望行为

Capability negotiation、handle owner/generation、event replay、close 和错误分类保持协议边界；SDK identity inventory 只监控漂移。

## 触发、结果与副作用

stdio/HTTP/WebSocket/webhook/SDK call 触发 Session、Turn、Tool 或 external delivery，并生成标准或 namespaced wire projection。

## 失败、重试与恢复

未协商能力、stale handle、unknown operation、disconnect、backpressure 和 EOF 需显式失败或恢复，不能 fallback 到隐藏本地执行。

## 证据

ACP/A2A/Channel/Headless/SDK Host 源码、Host E2E、ADR 0028/0031 与现有 SDK semantic map 提供证据。

## 裁决记录

用户确认 SDK identity 不再作为全项目完成门禁，现有 SDK map 作为本边界子图保留；A2A/Channel route 保持 needs_review。
