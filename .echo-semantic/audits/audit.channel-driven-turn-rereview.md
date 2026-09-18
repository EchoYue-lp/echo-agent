---
schema_version: 1
id: audit.channel-driven-turn-rereview
kind: audit
boundary_ref: boundary.agent-session-turn
lens: state_authority
freshness: examined
revision: c68a2c64a21c52315e8388dd4532477db9323d7b
finding_refs: [finding.turn-driver-entry-coverage]
challenges:
  session-reset-settlement:
    revision: c68a2c64a21c52315e8388dd4532477db9323d7b
    source_refs: [src/channels.rs, echo-integration/src/channels/session.rs, echo-integration/src/channels/types.rs]
    evidence_refs: [evidence.channel-driven-turn-repair, evidence.channel-driven-turn-verification]
  sink-and-transport-boundary:
    revision: c68a2c64a21c52315e8388dd4532477db9323d7b
    source_refs: [src/channels.rs, echo-integration/src/channels/channels/mod.rs]
    evidence_refs: [evidence.channel-driven-turn-repair, evidence.channel-driven-turn-verification]
---

# Channel driven Turn 独立复审

## 审查范围

独立 reviewer 检查 framework AgentChannelHandler、SessionHandler reset、QQ/Feishu wrapper、Turn driver、取消与 sink delivery 合同。

## 已检查故障假设

检查 no-op sink 被误报为远端送达、reset 丢弃正在驱动的 Turn、Notify 丢失唤醒、counter underflow、未轮询 stream 复活、legacy handler 行为回归和 transport close owner 越界。

## 实际实现路径与证据

AgentChannelHandler 使用 AgentTurnDriver 并可接受真实 EventSink；默认 sink 只表示进程内接纳。Session generation 将 cancellation 传给 opt-in driven handler，并在 reset 确认前等待 receipt 释放。admission/counter 共用状态锁，Notify 先注册再检查，legacy handler 保持原路径。

## 问题记录

首轮两个 Important 问题已修复；增量复审结论 pass。

## 残余风险

ChannelManager stop_all 与 QQ/Feishu adapter task 的关闭结算属于 Finding #36；本修复不引入第二 close coordinator。

## 未检查项

未执行真实 QQ/Feishu 网络 ACK 故障注入或 #36 的跨 adapter close 验收。
