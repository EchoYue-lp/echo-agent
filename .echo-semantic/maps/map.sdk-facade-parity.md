---
schema_version: 1
id: map.sdk-facade-parity
kind: capability_map
title: 多语言 SDK facade 对等边界
risk: high
observed_at: source:a298b808735ab2ddd9f004d60954a526e2f7673da044b942cb08c1f3228d31ca
boundary_refs: [boundary.sdk-facade-parity]
behavior_refs: [behavior.sdk-facade-routing]
rule_refs: [rule.sdk-rust-authority]
evidence_refs: [evidence.sdk-contracts]
finding_refs: [finding.sdk-component-stream-terminal, finding.sdk-sandbox-cancellation, finding.sdk-mcp-publication-cleanup, finding.sdk-skill-load-policy-bridge, finding.sdk-no-bridge-warnings]
audit_refs: [audit.sdk-facade-plan08-final]
related_map_refs: []
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
    evidence_refs: [evidence.sdk-contracts]
  extension-bridge:
    status: mapped
    source_refs: [echo-sdk-host/src/core_profile/extension_bridge.rs]
    evidence_refs: [evidence.sdk-contracts]
  facade-stream:
    status: mapped
    source_refs: [echo-sdk-host/src/core_profile/facade/stream.rs]
    behavior_refs: [behavior.sdk-facade-routing]
    evidence_refs: [evidence.sdk-contracts]
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

正式design与ADR 0028定义边界；parity manifest和operation catalog定义机器清单；Rust服务定义运行语义。

## 生命周期与失败路径

未知operation、错误signature、缺失feature、stale handle、断连、timeout和backpressure均fail closed；EOF不构成成功终态。

## 权限与敏感信息

permission operation复用Session Agent的`PermissionService`。Host不得引入额外线上权限门控，密钥不得进入协议诊断。

## 用户侧投影

三语言使用惯用Promise/iterator/CompletionStage等API，但必须保留同一identity、错误、取消与终态事实。

## 场景处置清单

ACP、core与extension已有真实Host证据；Plan 8 focused测试证明source operation逐项命中adapter，live consumer trait进入typed compressor或AgentComponent bridge，Workflow/A2A stream复用统一HandleRegistry并覆盖Session/connection teardown。

## 未展开项

intrinsic 语言行为、逐项领域/失败语义与最终 Parity complete 属于后续独立交付结果；
全仓 inventory/behavior model 和 clean-checkout 发布证据也仍保持开放。
