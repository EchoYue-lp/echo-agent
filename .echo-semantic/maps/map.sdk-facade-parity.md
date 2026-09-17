---
schema_version: 1
id: map.sdk-facade-parity
kind: capability_map
title: 外部 SDK consumer 边界
risk: high
observed_at: source:bfa5b4590c617d8286f2b80571d5c450622d47c7425d6f1ecb1978bf85743352
boundary_refs: [boundary.sdk-facade-parity]
behavior_refs: [behavior.protocol-projection, behavior.sdk-facade-routing]
rule_refs: [rule.protocol-role-separation, rule.framework-layer-ownership, rule.sdk-rust-authority]
evidence_refs: [evidence.sdk-repository-extraction-equivalence, evidence.sdk-repository-extraction-verification, evidence.checkpoint-journal-sdk-inventory]
finding_refs: [finding.sdk-component-stream-terminal, finding.sdk-sandbox-cancellation, finding.sdk-mcp-publication-cleanup, finding.sdk-skill-load-policy-bridge, finding.sdk-no-bridge-warnings, finding.sdk-gap-ack-replay-watermark, finding.sdk-repository-extraction]
audit_refs: [audit.sdk-facade-plan08-final, audit.sdk-facade-scope-contract, audit.sdk-repository-extraction-rereview]
related_map_refs: [map.protocol-surfaces]
scenarios:
  standard-acp:
    status: mapped
    source_refs: [src/acp/adapter.rs]
    behavior_refs: [behavior.protocol-projection]
  core-profile:
    status: excluded
    source_refs: [docs/adr/0051-extract-sdk-repository.md, README.md]
    reason: SDK core profile is owned and verified in the independent echo-agent-sdk repository
    risk: local framework documentation could accidentally recreate an SDK authority
    recheck_when: framework intentionally resumes ownership of the SDK core profile
  source-operation-closure:
    status: excluded
    source_refs: [docs/adr/0051-extract-sdk-repository.md, README.md]
    reason: SDK facade operation closure moved to the external SDK product
    risk: framework changes must not silently promise language operation parity
    recheck_when: an accepted external contract explicitly adds a framework-owned projection
  extension-bridge:
    status: excluded
    source_refs: [docs/adr/0051-extract-sdk-repository.md, README.md]
    reason: SDK reverse extension bridge is maintained by the external SDK product
    risk: a second bridge implementation could diverge from framework runtime authority
    recheck_when: a future framework boundary change assigns bridge ownership here
  facade-stream:
    status: excluded
    source_refs: [docs/adr/0051-extract-sdk-repository.md, README.md]
    reason: SDK facade stream projections are external consumer behavior
    risk: framework docs could confuse projections with stream lifecycle authority
    recheck_when: framework exposes a new generic stream adapter contract
  sdk-contract-scope:
    status: excluded
    source_refs: [docs/adr/0051-extract-sdk-repository.md, README.md]
    reason: accepted external contract and Rust inventory telemetry are owned by echo-agent-sdk
    risk: framework public API drift must not become an implicit SDK compatibility gate
    recheck_when: the external contract ownership decision changes
  gap-ack-replay-watermark:
    status: excluded
    source_refs: [docs/adr/0051-extract-sdk-repository.md, README.md]
    finding_refs: [finding.sdk-gap-ack-replay-watermark]
    reason: the unresolved SDK replay watermark scenario moved with its Host and protocol owner
    risk: the external SDK must retain the open Finding and its end-to-end counterexample
    recheck_when: SDK replay ownership returns to the framework
---

# 外部 SDK consumer 边界

## 能力范围

当前 framework map 只记录通用 ACP adapter；SDK core、facade、extension、stream 和 contract
场景由独立 `echo-agent-sdk` 仓库维护。

## 入口与输出

framework 入口是 `src/acp/adapter.rs`；外部 SDK 入口、typed operations、streams 和 bridge
不在本仓实现。

## 行为关系

ACP adapter 投影 framework Agent/Session/Turn，不拥有 SDK facade 或语言 parity 状态。

## 状态与数据流

Agent、Run、Task、Subagent、event、retry、cancel、recovery 和 terminal 仍由 framework authority
持有；SDK Host 只消费这些事实。

## 策略来源与优先级

ADR 0051 规定仓库 ownership；外部 SDK 的 ADR 0001、0031、0032 和 accepted contract 约束 SDK
兼容面，不能被 framework 文档覆盖。

## 生命周期与失败路径

通用 ACP lifecycle 在 framework tests 验证；SDK core/bridge/replay 失败路径在外部仓复核。

## 权限与敏感信息

framework 复用既有 Permission/Sandbox；SDK 不在 framework 中新增权限闸或凭据路径。

## 用户侧投影

外部 SDK 语言 API 是 consumer projection，不成为 Rust runtime 或 framework map 的第二权威。

## 场景处置清单

一个 standard ACP 场景保持 mapped；六个 SDK-owned 场景明确 excluded，并记录风险、复查条件与
尚未关闭的 gap ACK replay Finding。历史 SDK Finding 继续留在 framework 语义账本，外部 SDK
建立对应 owner 后再通过独立修复关闭，不能因源码迁移而消失。

## 未展开项

SDK 侧的 protocol purity、external contract、Host pin 和三语言门禁在独立仓 outcome 中展开。
