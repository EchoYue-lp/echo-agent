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
observed_at: source:e2f708d5ddfba82cdb921b3559e440532246d4f9b679a4fa104b7f8f173b3a6b
code_refs: [src/acp/adapter.rs, src/acp/runtime.rs, src/a2a/server.rs, echo-integration/src/channels/manager.rs, echo-integration/src/lsp/manager.rs, src/channels.rs, src/headless.rs]
rule_refs: [rule.protocol-role-separation, rule.sdk-rust-authority]
evidence_refs: [evidence.provider-protocol-quality, evidence.sdk-contracts, evidence.lsp-derived-handle-lifecycle-repair, evidence.lsp-derived-handle-lifecycle-verification, evidence.channel-generation-delivery-fence-repair, evidence.channel-generation-delivery-fence-verification]
finding_refs: [finding.a2a-terminal-authority, finding.a2a-stream-cleanup, finding.channel-attachment-projection, finding.turn-driver-entry-coverage, finding.lsp-manager-derived-handle-resurrection]
---

# Protocol 与 Surface 投影

## 重要承诺

ACP 连接 Client 与 coding Agent，MCP 连接 Agent 与 tools/resources，A2A 连接 Agent 与 Agent；协议 adapter 应投影框架事实，不形成第二执行模型。

## 当前行为

ACP Session/Prompt/update/cancel 投影同一 driven Agent Turn，并在关闭时保留无receipt的Run；A2A保持当前自持task/terminal与开放Finding；Channels以generation fence阻止reset后的旧delivery，并由AgentChannelHandler进入driven Turn、SessionHandler关闭sender Agent；Headless聚合driven Turn且在返回前await Agent close；SDK Host以ACP和`_echo_agent/*`暴露Rust authority。

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
