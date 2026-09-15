---
schema_version: 1
id: map.sdk-facade-parity
kind: capability_map
title: 多语言 SDK facade 对等边界
risk: high
observed_at: source:b0dfa235d236bee9185ff26953b982f196556459fd0bf165f7114a232373b224
boundary_refs: [boundary.sdk-facade-parity]
behavior_refs: [behavior.sdk-facade-routing]
rule_refs: [rule.sdk-rust-authority]
evidence_refs: [evidence.sdk-contracts, evidence.tool-registry-owned-handle-verification, evidence.background-task-terminal-authority-verification, evidence.framework-concept-navigation]
finding_refs: [finding.sdk-component-stream-terminal, finding.sdk-sandbox-cancellation, finding.sdk-mcp-publication-cleanup, finding.sdk-skill-load-policy-bridge, finding.sdk-no-bridge-warnings, finding.sdk-gap-ack-replay-watermark]
audit_refs: [audit.sdk-facade-plan08-final, audit.sdk-facade-scope-contract]
related_map_refs: [map.protocol-surfaces]
scenarios:
  standard-acp:
    status: mapped
    source_refs: [src/acp/adapter.rs]
    behavior_refs: [behavior.sdk-facade-routing]
  core-profile:
    status: mapped
    source_refs: [echo-sdk-host/src/core_profile/handler.rs]
    rule_refs: [rule.sdk-rust-authority]
  source-operation-closure:
    status: mapped
    source_refs: [echo-sdk-host/src/core_profile/facade/source_operations.rs]
    behavior_refs: [behavior.sdk-facade-routing]
    evidence_refs: [evidence.sdk-contracts, evidence.tool-registry-owned-handle-verification]
  extension-bridge:
    status: mapped
    source_refs: [echo-sdk-host/src/core_profile/extension_bridge.rs]
    evidence_refs: [evidence.sdk-contracts]
  facade-stream:
    status: mapped
    source_refs: [echo-sdk-host/src/core_profile/facade/stream.rs]
    behavior_refs: [behavior.sdk-facade-routing]
    evidence_refs: [evidence.sdk-contracts]
  sdk-contract-scope:
    status: mapped
    source_refs: [echo-sdk-protocol/src/inventory.rs, contracts/sdk/parity-manifest.schema.json, contracts/sdk/parity-manifest.json, echo-sdk-protocol/tests/facade_inventory.rs, scripts/check-language-sdks.sh, docs/adr/0032-sdk-contract-scope-classification.md, docs/adr/0039-background-task-terminal-authority.md]
    behavior_refs: [behavior.sdk-facade-routing]
    rule_refs: [rule.sdk-rust-authority]
    evidence_refs: [evidence.sdk-contracts, evidence.background-task-terminal-authority-verification]
  gap-ack-replay-watermark:
    status: needs_review
    source_refs: [echo-sdk-host/src/core_profile/events.rs, echo-sdk-protocol/src/event.rs, echo-sdk-host/tests/core_profile_e2e.rs]
    finding_refs: [finding.sdk-gap-ack-replay-watermark]
    unknown: Client确认gap snapshot watermark后，Host resume watermark仍可能回退到旧live ACK并重发已被snapshot覆盖的事件
    next_step: 统一gap ACK与resume watermark的单调权威，并补gap到ACK再到replay/live continuation的端到端反例
---

# 多语言 SDK facade 对等边界

## 能力范围

覆盖根`echo_agent` facade经稳定ACP v1及`_echo_agent/*`扩展暴露给TypeScript、Python和Java的功能与语义。

## 入口与输出

入口是标准ACP方法、typed core/family方法、`_echo_agent/facade/invoke`与双向extension请求；输出是typed response、`EventEnvelope`、stream或稳定错误。

## 行为关系

标准ACP、core profile、feature family、source operation和extension bridge只投影既有Rust能力，不互相替代或形成fallback。

## 状态与数据流

Host只保存寻址、handle owner/generation和有界投递状态；Agent、Run、Task、Subagent、重试、取消与终态继续由框架服务持有。

## 策略来源与优先级

正式design与ADR 0028定义运行边界；ADR 0031/0032定义inventory与consumer scope；parity manifest和operation catalog定义机器清单；Rust服务定义运行语义。

## 生命周期与失败路径

未知operation、错误signature、缺失feature、stale handle、断连、timeout和backpressure均fail closed；EOF不构成成功终态。

## 权限与敏感信息

permission operation复用Session Agent的`PermissionService`。Host不得引入额外线上权限门控，密钥不得进入协议诊断。

## 用户侧投影

三语言使用惯用Promise/iterator/CompletionStage等API，但必须保留同一identity、错误、取消与终态事实。

## 场景处置清单

ACP、core与extension已有真实Host证据；Plan 8 focused测试证明source operation逐项命中adapter。Manifest schema v2以identity级`sdk_scope`区分5620个当前external contract、1773个Host/Rust-only、787个language intrinsic、90个internal helper与1443个deferred，总量9713；551个已完成intrinsic仍属于external contract。Gap generation校验已闭合，gap ACK后的replay watermark仍保持needs_review。

## 未展开项

deferred identity只按externally useful capability进入后续合同决策；Host/Rust-only、language intrinsic和helper不是逐identity语言任务。全仓inventory/behavior model由父级能力图闭合，不以本map的identity数量衡量。
