---
schema_version: 1
id: rule.protocol-role-separation
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus:
- contract_evidence
- permission_external
- time_lifecycle
observed_at: source:1c515c09873968a279e6076cf6abf659923f1fa96137144423970bfcd0e1881c
behavior_refs:
- behavior.protocol-projection
code_refs:
- src/acp/adapter.rs
- src/a2a/server.rs
- echo-integration/src/mcp/mod.rs
- echo-integration/src/lsp/manager.rs
- echo-integration/src/channels/manager.rs
- src/channels.rs
- src/headless.rs
- docs/adr/0028-source-first-multilanguage-sdk-runtime.md
- docs/adr/0043-lsp-derived-handle-lifecycle.md
- src/acp/extension.rs
- docs/adr/0060-task-execution-store-cas-and-local-interruption.md
- src/acp/session.rs
- echo-core/src/lsp/types.rs
- echo-integration/src/lsp/client.rs
- echo-integration/src/lsp/jsonrpc.rs
- src/tools/lsp.rs
- docs/adr/0061-acp-recovered-turn-identity.md
- docs/adr/0082-lsp-response-value-fidelity.md
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



# Protocol 角色分离

## 不变量或唯一权威

ExtensionInvocationAuthority 仅拥有 invocation 生命周期；lease 只能取消自己的 child，不能把局部取消升级为 run/connection cancellation，也不拥有 Task/Agent terminal。

ACP 是 Client-Agent 协议，MCP 是 Agent-capability 协议，A2A 是 Agent-Agent 协议；Channels/Headless/SDK 是入口或投影，不拥有第二核心 runtime。

## 适用行为

适用于 Session/Run/Task wire、capability negotiation、handle/replay、tool/resource 调用、外部消息与 source SDK。

## 当前实现

ACP/SDK Host 复用Turn/Session authorities，MCP适配Tool/Resource，Headless聚合TurnReceipt并保留
close owner；Channels的reset delivery复用SessionGeneration并在transport边界fence旧代，Agent close
owner已闭合但raw Agent chat入口仍待收敛；A2A自持terminal仍为待审反例。

## 期望行为

协议 adapter 不重算通用 terminal/retry/recovery；扩展数据使用 namespaced contract；缺 capability 不 fallback 到隐藏执行。

## 证据

ADR 0028、ACP/SDK E2E、MCP/A2A/Channel/Headless 实现和现有 SDK map 提供证据。

## 裁决记录

SDK identity inventory 的项目治理边界由 ADR 0031 确认；A2A/Channel 是否应投影 driven Turn 仍待 audit，故本 Rule 保持 needs_review。

组合候选补充范围：ACP 恢复身份预留仍由 Session TurnSlot 唯一分配；LSP response/diagnostic 由 framework 解析，URI 单缓存区分有效结果与明确错误，fatal admission 后不返回假空。当前只有指定切片的局部复审和反例证据，组合 23 项门禁运行中；SDK 已发布 pin、其它协议缺口和 main 仍各自验收。
