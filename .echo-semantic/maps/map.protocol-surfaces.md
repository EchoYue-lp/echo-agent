---
schema_version: 1
id: map.protocol-surfaces
kind: capability_map
title: ACP、A2A、Channels、Headless 与 SDK Surfaces
risk: high
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
boundary_refs: [boundary.protocol-surfaces]
behavior_refs: [behavior.protocol-projection]
rule_refs: [rule.protocol-role-separation, rule.sdk-rust-authority]
evidence_refs: [evidence.provider-protocol-quality, evidence.sdk-contracts]
finding_refs: [finding.a2a-terminal-authority, finding.a2a-stream-cleanup, finding.channel-attachment-projection, finding.turn-driver-entry-coverage]
audit_refs: []
related_map_refs: [map.agent-session-turn, map.observation-persistence-delivery, map.extension-lifecycle, map.sdk-facade-parity]
scenarios:
  acp-session-run-projection:
    status: mapped
    source_refs: [src/acp/session.rs, src/acp/runtime.rs, src/acp/projection.rs]
    behavior_refs: [behavior.protocol-projection]
    rule_refs: [rule.turn-terminal-authority, rule.protocol-role-separation]
  a2a-task-and-stream:
    status: mapped
    source_refs: [src/a2a/server.rs, src/a2a/types.rs]
    finding_refs: [finding.a2a-terminal-authority, finding.a2a-stream-cleanup]
  channel-session-and-message:
    status: needs_review
    source_refs: [echo-integration/src/channels/session.rs, echo-integration/src/channels/types.rs, src/channels.rs]
    finding_refs: [finding.channel-attachment-projection, finding.turn-driver-entry-coverage]
    evidence_refs: [evidence.provider-protocol-quality]
    unknown: Channel session/incarnation 已映射，但 handler 丢弃 attachments 且直接调用 raw Agent chat，不产生 driven TurnReceipt
    next_step: protocol/turn audit 裁决 Channel adapter 的 multimodal 与 terminal projection contract
  headless-turn:
    status: mapped
    source_refs: [src/headless.rs, echo-orchestration/src/runtime/turn_driver.rs]
    behavior_refs: [behavior.protocol-projection]
    rule_refs: [rule.turn-terminal-authority]
  sdk-host-and-language-clients:
    status: mapped
    source_refs: [echo-sdk-protocol/src/lib.rs, echo-sdk-host/src/lib.rs, contracts/sdk/parity-manifest.json]
    behavior_refs: [behavior.sdk-facade-routing]
    rule_refs: [rule.sdk-rust-authority]
    evidence_refs: [evidence.sdk-contracts]
  sdk-intrinsic-backlog:
    status: needs_review
    source_refs: [contracts/sdk/parity-manifest.json, docs/adr/0031-sdk-identity-governance-scope.md]
    unknown: 4076 个 intrinsic identity 的 capability 分组与外部用户价值尚未形成独立 SDK backlog
    next_step: SDK contract scope outcome 按 public capability、Host/Rust-only、language intrinsic、internal helper 与 deferred 分类
  product-backend-desktop-device:
    status: excluded
    source_refs: [docs/en/39-framework-application-boundary.md]
    reason: Product Backend、Frontend/Desktop、Canvas、Electron/Tauri 与 Device sync 不属于 echo-agent framework 当前源码
    risk: high
    recheck_when: 出现 product-neutral protocol primitive 且不依赖应用 state/UI 时
---

# ACP、A2A、Channels、Headless 与 SDK Surfaces

## 能力范围

覆盖外部 Client/Agent/peer/channel/headless/source SDK 入口及其 Session/Run/event/handle projection。

## 入口与输出

ACP stdio、A2A HTTP/SSE、IM WebSocket/webhook、Headless prompt 和 SDK process client 进入；输出标准或 namespaced wire 与外部 delivery。

## 行为关系

ACP、MCP、A2A 角色分离；Headless/SDK Host 复用 driven Turn；Channel 当前只复用 raw Agent execution，A2A 当前自持 task/terminal。

## 状态与数据流

Session registry、A2A task map、Channel incarnation、SDK handle/generation 分别提供寻址；通用 terminal 应来自 framework runtime。

## 策略来源与优先级

Protocol capability negotiation、typed config、JWT/channel credentials、SDK contract 与 application adapter 决定可用 surface。

## 生命周期与失败路径

Initialize/create/start/stream/cancel/replay/close/recover；unknown method、stale handle、gap、disconnect 和 backpressure 显式处理。

## 权限与敏感信息

ACP permission 与 Agent policy 只在协商后调用；channel/provider credentials 不进入 event；本地协议不套多租户假设。

## 用户侧投影

各 surface 可有不同 wire/UI，但功能与通用生命周期事实不得分叉。

## 场景处置清单

五类入口均有路由；A2A/Channel 保持 needs_review 并进入 Finding，SDK intrinsic backlog needs_review，产品层 excluded。

## 未展开项

现有 `map.sdk-facade-parity` 保留完整 SDK 子边界，不在本 map 重复 identity。
