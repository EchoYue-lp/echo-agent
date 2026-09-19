---
schema_version: 1
id: map.protocol-surfaces
kind: capability_map
title: ACP、A2A、Channels、Headless 与 SDK Surfaces
risk: high
observed_at: source:17f0054af370c86c5f9dbca52db70bcaa417b1f08403153c73b7c0fc4e23c4b8
boundary_refs: [boundary.protocol-surfaces]
behavior_refs: [behavior.protocol-projection]
rule_refs: [rule.protocol-role-separation, rule.sdk-rust-authority]
evidence_refs: [evidence.provider-protocol-quality, evidence.sdk-contracts, evidence.high-risk-audit-frontier, evidence.framework-concept-navigation, evidence.sdk-deferred-backlog-count-repair, evidence.sdk-deferred-backlog-count-verification]
finding_refs: [finding.a2a-terminal-authority, finding.a2a-stream-cleanup, finding.a2a-task-id-admission-authority, finding.a2a-advertised-capability-binding, finding.channel-attachment-projection, finding.channel-reset-stale-generation-delivery, finding.turn-driver-entry-coverage, finding.agent-adapter-close-settlement, finding.turn-terminal-commit-projection-order, finding.sdk-gap-generation-validation-parity, finding.sdk-gap-ack-replay-watermark, finding.sdk-deferred-backlog-count-drift]
audit_refs: [audit.protocol-surfaces.state-authority, audit.protocol-surfaces.time-lifecycle, audit.protocol-surfaces.contract-evidence, audit.semantic-governance-final-rereview]
related_map_refs: [map.agent-session-turn, map.observation-persistence-delivery, map.extension-lifecycle, map.sdk-facade-parity]
scenarios:
  acp-session-run-projection:
    status: mapped
    source_refs: [src/acp/session.rs, src/acp/runtime.rs, src/acp/projection.rs]
    behavior_refs: [behavior.protocol-projection]
    rule_refs: [rule.turn-terminal-authority, rule.protocol-role-separation]
  a2a-task-and-stream:
    status: needs_review
    source_refs: [src/a2a/server.rs, src/a2a/types.rs]
    finding_refs: [finding.a2a-terminal-authority, finding.a2a-stream-cleanup, finding.a2a-task-id-admission-authority, finding.a2a-advertised-capability-binding]
    unknown: A2A 自持 terminal、重复 ID 无 generation、stream cleanup 与宣告 capability 均未闭合
    next_step: 分别 repair admission/cleanup/terminal，并 semantic-decide text+SSE 或 file/push 合同
  channel-session-and-message:
    status: needs_review
    source_refs: [echo-integration/src/channels/session.rs, echo-integration/src/channels/types.rs, src/channels.rs]
    finding_refs: [finding.channel-attachment-projection, finding.channel-reset-stale-generation-delivery, finding.turn-driver-entry-coverage, finding.agent-adapter-close-settlement]
    evidence_refs: [evidence.provider-protocol-quality, evidence.channel-generation-delivery-fence-repair, evidence.channel-generation-delivery-fence-verification]
    unknown: Channel session/incarnation与reset delivery fence已映射，但handler仍丢弃attachments、直接raw chat且close不结算Agent
    next_step: 分别repair attachment投影、driven Turn入口与close owner；reset generation fencing已闭合待交付
  headless-turn:
    status: mapped
    source_refs: [src/headless.rs, echo-orchestration/src/runtime/turn_driver.rs]
    behavior_refs: [behavior.protocol-projection]
    rule_refs: [rule.turn-terminal-authority]
  sdk-host-and-language-clients:
    status: needs_review
    source_refs: [.echo-semantic/assets/asset.external-sdk-repository.md, docs/adr/0031-sdk-identity-governance-scope.md]
    behavior_refs: [behavior.sdk-facade-routing]
    rule_refs: [rule.sdk-rust-authority]
    evidence_refs: [evidence.sdk-contracts]
    finding_refs: [finding.sdk-gap-generation-validation-parity, finding.sdk-gap-ack-replay-watermark]
    unknown: Gap generation校验已统一，但Client确认snapshot watermark后，Host resume watermark仍可能回退并重发旧事件
    next_step: Repair gap ACK后的单调resume watermark，不扩大到intrinsic identity门禁
  sdk-deferred-backlog:
    status: needs_review
    source_refs: [.echo-semantic/assets/asset.external-sdk-repository.md, docs/adr/0031-sdk-identity-governance-scope.md, docs/adr/0032-sdk-contract-scope-classification.md]
    evidence_refs: [evidence.sdk-deferred-backlog-count-repair, evidence.sdk-deferred-backlog-count-verification]
    finding_refs: [finding.sdk-deferred-backlog-count-drift]
    audit_refs: [audit.semantic-governance-final-rereview]
    unknown: 1448个deferred identity的capability分组、外部用户价值与逐组产品合同决策尚未闭合
    next_step: 按externally useful capability审查deferred；Host/Rust-only、language intrinsic与internal helper不是语言parity backlog
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

五类入口均有路由；A2A/Channel/SDK gap保持needs_review并进入Finding，SDK deferred capability backlog独立needs_review，产品层excluded。

## 未展开项

现有 `map.sdk-facade-parity` 保留完整 SDK 子边界，不在本 map 重复 identity。
