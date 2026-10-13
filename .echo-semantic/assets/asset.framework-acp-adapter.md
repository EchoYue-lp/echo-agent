---
schema_version: 1
id: asset.framework-acp-adapter
kind: asset
title: Framework 通用 ACP Agent adapter
asset_type: protocol
status: active
risk: high
observed_at: source:34d5f5574c9cf5a645970cd8b58367c39bce268630488b68d3ea54e996728e17
boundary_refs:
- boundary.protocol-surfaces
- boundary.workspace-architecture
code_refs:
- src/acp/adapter.rs
- src/acp/runtime.rs
- src/acp/projection.rs
- src/acp/mod.rs
- tests/acp_agent_adapter.rs
- tests/fixtures/acp/v1/prompt-resource-link-valid.json
- tests/fixtures/acp/v1/session-relative-cwd-invalid.json
- src/acp/extension.rs
- docs/adr/0060-task-execution-store-cas-and-local-interruption.md
consumer_refs:
- tests/acp_agent_adapter.rs
- echo-agent-learning/examples/demo72_acp_agent_adapter.rs
behavior_refs:
- behavior.protocol-projection
- behavior.agent-turn-lifecycle
rule_refs:
- rule.protocol-role-separation
- rule.turn-terminal-authority
- rule.framework-layer-ownership
evidence_refs:
- evidence.provider-protocol-quality
- evidence.workspace-structure
- evidence.agent-adapter-close-settlement-repair
- evidence.agent-adapter-close-settlement-verification
- evidence.task-runtime-store-control-repair
- evidence.task-runtime-store-control-verification
- evidence.lsp-response-value-fidelity-investigation
- evidence.lsp-diagnostic-notification-repair-verification
finding_refs:
- finding.sdk-repository-extraction
- finding.extension-invocation-cancellation-scope
- finding.lsp-response-value-fidelity
- finding.lsp-diagnostic-notification-fidelity
candidate_refs:
- asset.sdk-source-product
---


# Framework 通用 ACP Agent adapter

## 资产身份

产品无关的 ACP Agent role、Session/Prompt 投影、取消、连接运行时和 framework event
转换；它是 embedding application 与 SDK Host 都可以消费的通用 framework 边界。

## 来源与消费者

由 `echo_agent` root facade 提供，framework conformance tests、learning example、未来
独立 SDK Host 和其它 ACP consumers 消费。

## 生命周期

Initialize、Session、Prompt、update、cancel、receipt 和 connection close 继续由 framework
runtime authority 负责；adapter 不拥有第二个 Agent/Run 状态机。

## 候选关系

它是 SDK source product 的 replacement/canonical framework dependency，不随 SDK 资产删除。

## 未知与限制

具体 SDK Host 对 protocol DTO 的转换将在独立仓库中继续维护；framework 只保证通用 ACP
adapter 的行为和接口，不拥有 SDK language parity。

组合候选补充范围：ACP 恢复身份预留仍由 Session TurnSlot 唯一分配；LSP response/diagnostic 由 framework 解析，URI 单缓存区分有效结果与明确错误，fatal admission 后不返回假空。当前只有指定切片的局部复审和反例证据，组合 23 项门禁运行中；SDK 已发布 pin、其它协议缺口和 main 仍各自验收。
