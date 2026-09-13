---
schema_version: 1
id: audit.protocol-surfaces.state-authority
kind: audit
boundary_ref: boundary.protocol-surfaces
lens: state_authority
freshness: examined
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.a2a-terminal-authority, finding.a2a-task-id-admission-authority, finding.turn-driver-entry-coverage, finding.channel-attachment-projection, finding.turn-terminal-commit-projection-order]
challenges:
  surface-authority-matrix:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/acp/session.rs, src/acp/runtime.rs, src/headless.rs, src/a2a/server.rs, src/a2a/types.rs, src/channels.rs, echo-integration/src/channels/session.rs, echo-sdk-host/src/core_profile/handler.rs]
    evidence_refs: [evidence.provider-protocol-quality, evidence.agent-context-execution]
  a2a-terminal-and-admission:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/a2a/server.rs, src/a2a/types.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  channel-turn-and-attachments:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/channels.rs, echo-integration/src/channels/types.rs, echo-integration/src/channels/session.rs]
    evidence_refs: [evidence.provider-protocol-quality]
---

# ACP、A2A、Channel、Headless 与 SDK 状态权威审计

## 审查范围

审查各 surface 的 address/session、execution/terminal 与 projection/persistence authority，并区分固定 wire state 与内部 runtime。

## 已检查故障假设

验证 ACP/Headless/SDK 是否共享 driven Turn，A2A 是否以 wire reducer替代内部 terminal，Channel 是否缺 Turn identity/attachment，以及重复 A2A task ID 是否覆盖新 generation。

## 实际实现路径与证据

ACP 使用 SessionRegistry、single ActiveTurnLease、AgentTurnDriver 与 EventLedger；SDK Host 复用 ACP live receipt并用 HandleRegistry generation；Headless 聚合 TurnReceipt。A2A 自持 tasks/cancel_tokens/TaskState 并直接消费 raw stream，stream completion 可覆盖 Canceled。相同 client task ID 可无条件覆盖 task/token，旧执行再更新/删除新代。Channel session incarnation 只管理 handler lifecycle，adapter raw chat 无 Turn identity/receipt/cancel并丢弃 attachments。

## 问题记录

确认 A2A terminal、Channel Turn/attachment 和 terminal commit order；新增 A2A task admission authority。Raw Rust Agent API 保留为合理低层能力。

## 残余风险

A2A TaskState 可保留为 wire projection，但内部 terminal/attempt 必须绑定通用 execution identity；Channel multimodal与Turn contract需产品裁决。

## 未检查项

未校验外部最新 A2A 规范，未做真实网络/SSE/媒体或应用 Channel adapter验收。
