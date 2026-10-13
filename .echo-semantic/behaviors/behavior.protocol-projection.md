---
schema_version: 1
id: behavior.protocol-projection
kind: behavior
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: contract_evidence
focus:
- state_authority
- time_lifecycle
- permission_external
- failure_concurrency
boundary: boundary.protocol-surfaces
observed_at: source:1c515c09873968a279e6076cf6abf659923f1fa96137144423970bfcd0e1881c
code_refs:
- src/acp/adapter.rs
- src/acp/runtime.rs
- src/a2a/server.rs
- echo-integration/src/channels/manager.rs
- echo-integration/src/lsp/manager.rs
- src/channels.rs
- src/headless.rs
- src/acp/extension.rs
- docs/adr/0060-task-execution-store-cas-and-local-interruption.md
- src/acp/session.rs
- echo-core/src/lsp/types.rs
- echo-integration/src/lsp/client.rs
- echo-integration/src/lsp/jsonrpc.rs
- src/tools/lsp.rs
- docs/adr/0061-acp-recovered-turn-identity.md
- docs/adr/0082-lsp-response-value-fidelity.md
rule_refs:
- rule.protocol-role-separation
- rule.sdk-rust-authority
evidence_refs:
- evidence.provider-protocol-quality
- evidence.sdk-contracts
- evidence.lsp-derived-handle-lifecycle-repair
- evidence.lsp-derived-handle-lifecycle-verification
- evidence.channel-generation-delivery-fence-repair
- evidence.channel-generation-delivery-fence-verification
- evidence.agent-adapter-close-settlement-repair
- evidence.agent-adapter-close-settlement-verification
- evidence.task-runtime-store-control-repair
- evidence.task-runtime-store-control-verification
- evidence.lsp-response-value-fidelity-investigation
- evidence.lsp-diagnostic-notification-repair-verification
- evidence.framework-parent-drop-final-verification
finding_refs:
- finding.a2a-terminal-authority
- finding.a2a-stream-cleanup
- finding.channel-attachment-projection
- finding.turn-driver-entry-coverage
- finding.lsp-manager-derived-handle-resurrection
- finding.extension-invocation-cancellation-scope
- finding.lsp-response-value-fidelity
- finding.lsp-diagnostic-notification-fidelity
---



# Protocol 与 Surface 投影

## 重要承诺

ACP 连接 Client 与 coding Agent，MCP 连接 Agent 与 tools/resources，A2A 连接 Agent 与 Agent；协议 adapter 应投影框架事实，不形成第二执行模型。

## 当前行为

ACP 扩展 invocation lease 为传入 run/connection token 创建独立 child。父取消向下传播，单次流释放、timeout 或 invocation cancel 不取消父与 sibling；close_admission 与 cancel_all 保留连接关闭/全部 child 清理语义。SDK 不再需要平行 token 层实现该通用取消隔离。

ACP Session/Prompt/update/cancel 投影同一 driven Agent Turn，并在关闭时保留无receipt的Run；A2A保持当前自持task/terminal与开放Finding；Channels以generation fence阻止reset后的旧delivery，并由AgentChannelHandler进入driven Turn、SessionHandler关闭sender Agent；Headless通过owned task/handle聚合driven Turn并保留Agent close retry；SDK Host以ACP和`_echo_agent/*`暴露Rust authority。

## 期望行为

Capability negotiation、handle owner/generation、event replay、close 和错误分类保持协议边界；SDK identity inventory 只监控漂移。

## 触发、结果与副作用

stdio/HTTP/WebSocket/webhook/SDK call 触发 Session、Turn、Tool 或 external delivery，并生成标准或 namespaced wire projection。

## 失败、重试与恢复

未协商能力、stale handle、unknown operation、disconnect、backpressure 和 EOF 需显式失败或恢复，不能 fallback 到隐藏本地执行。

## 证据

ACP extension 旧实现的 3 条隔离反例失败，当前 13 条单元测试及 framework 完整门禁通过；本 Evidence 不证明 SDK transport 三语言合同或其它 A2A/Channel 生命周期缺口。

ACP/A2A/Channel/Headless/SDK Host 源码、Host E2E、ADR 0028/0031 与现有 SDK semantic map 提供证据。

## 裁决记录

用户确认 SDK identity 不再作为全项目完成门禁，现有 SDK map 作为本边界子图保留；A2A/Channel route 保持 needs_review。

组合候选补充范围：ACP 恢复身份预留仍由 Session TurnSlot 唯一分配；LSP response/diagnostic 由 framework 解析，URI 单缓存区分有效结果与明确错误，fatal admission 后不返回假空。当前只有指定切片的局部复审和反例证据，组合 23 项门禁运行中；SDK 已发布 pin、其它协议缺口和 main 仍各自验收。
