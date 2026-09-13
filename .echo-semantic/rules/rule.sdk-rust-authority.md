---
schema_version: 1
id: rule.sdk-rust-authority
kind: rule
status: verified
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [contract_evidence, failure_concurrency, time_lifecycle]
observed_at: source:13192164b42c8866c7eefcb6085ce026369709d5eec4500ab6c94bb285436519
behavior_refs: [behavior.sdk-facade-routing]
code_refs:
  - docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md
  - docs/adr/0028-source-first-multilanguage-sdk-runtime.md
  - echo-sdk-host/src/core_profile/handler.rs
  - echo-sdk-host/src/core_profile/handles.rs
  - echo-sdk-host/src/core_profile/facade/source_operations.rs
  - echo-sdk-host/src/core_profile/facade/stream.rs
evidence_refs: [evidence.sdk-contracts]
finding_refs: [finding.sdk-component-stream-terminal, finding.sdk-sandbox-cancellation, finding.sdk-mcp-publication-cleanup, finding.sdk-skill-load-policy-bridge, finding.sdk-no-bridge-warnings]
---

# Rust 是 SDK 唯一语义权威

## 不变量或唯一权威

Rust `echo-agent`继续唯一拥有Agent、Run、Task、Subagent、事件、重试、取消、恢复、permission和终态语义。

## 适用行为

适用于ACP标准投影、`_echo_agent/*` core/family/source operation、extension bridge及三语言Client。

## 当前实现

Host adapter只做typed值转换、handle寻址、generation fencing和调用现有服务；事件使用`EventEnvelope`，Run使用`AgentTurnDriver`及既有存储。文件资源只保留Rust lease/guard，stream业务map只保留receiver/background，HandleRegistry仍是唯一identity与生命周期权威。

## 期望行为

新增facade能力必须扩展现有权威；不得在Host或语言SDK建立第二状态机、重试器、终态归约或持久化事实源。

## 证据

设计、ADR、parity manifest、operation catalog及真实Host测试共同约束该边界。

## 裁决记录

用户已确认采用Rust Host方案和功能语义对等，排除FFI同构与多语言重写。
