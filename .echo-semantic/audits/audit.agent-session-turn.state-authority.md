---
schema_version: 1
id: audit.agent-session-turn.state-authority
kind: audit
boundary_ref: boundary.agent-session-turn
lens: state_authority
freshness: examined
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.turn-driver-entry-coverage, finding.agent-adapter-close-settlement]
challenges:
  raw-versus-driven-turn:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-core/src/agent/mod.rs, src/agent/react/run/react_loop.rs, echo-orchestration/src/runtime/turn_driver.rs, echo-core/src/agent/event_envelope.rs]
    evidence_refs: [evidence.agent-context-execution]
  channel-terminal-projection:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/channels.rs, echo-integration/src/channels/session.rs]
    evidence_refs: [evidence.agent-context-execution, evidence.provider-protocol-quality]
  adapter-close-settlement:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-integration/src/channels/manager.rs, echo-integration/src/channels/types.rs, src/agent/react/mod.rs]
    evidence_refs: [evidence.agent-context-execution]
---

# Agent、Session 与 driven Turn 状态权威审计

## 审查范围

审查 raw `Agent` execution、`AgentTurnDriver`/`TurnReceipt`、Channel adapter 与 Agent close owner；A2A 终态留在 protocol 专项 Audit。

## 已检查故障假设

验证 raw API 是否本身形成第二终态权威、Channel 是否缺 driven Turn identity/accounting，以及 adapter stop/drop 是否等待 Agent close 和资源清理。

## 实际实现路径与证据

Raw `Agent::execute/chat/stream` 是 driver 依赖的低层 public contract，并不承诺 TurnReceipt；ReactAgent 非流式路径能把 cancel/error/无终态 EOF 转为错误。Headless/ACP 使用 driver，Channel handler 只调用 raw `chat`，其 session generation receipt 不提供 Agent Turn terminal、usage 或 cancel identity。Channel stop 只停止 transport，MessageHandler 无 close 合同；ReactAgent Drop 的 MCP cleanup 也是 runtime 存在时的未等待任务。

## 问题记录

`finding.turn-driver-entry-coverage` 收窄为 Channel 外部 adapter 缺口，不把 direct Rust API 判错；新增 `finding.agent-adapter-close-settlement` 记录跨 adapter 的 Agent close/cleanup owner 缺口。

## 残余风险

Channel 是否必须拥有完整 TurnReceipt 属 adapter contract；低层 Agent API继续作为合理 framework primitive。A2A sync/stream terminal 差异由 protocol Finding 单独审计。

## 未检查项

未检查具体 QQ/飞书网络断开后的 handler ownership、应用层 Channel adapter、第三方 Agent 实现和真实 provider stall。
