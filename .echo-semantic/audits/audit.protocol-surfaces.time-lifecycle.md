---
schema_version: 1
id: audit.protocol-surfaces.time-lifecycle
kind: audit
boundary_ref: boundary.protocol-surfaces
lens: time_lifecycle
freshness: examined
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.a2a-stream-cleanup, finding.a2a-task-id-admission-authority, finding.agent-adapter-close-settlement, finding.turn-terminal-commit-projection-order, finding.channel-reset-stale-generation-delivery]
challenges:
  a2a-stream-cancel-and-drop:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/a2a/server.rs, src/a2a/serve.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  adapter-agent-close:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/acp/adapter.rs, src/acp/session.rs, src/headless.rs, src/a2a/server.rs, echo-integration/src/channels/manager.rs, echo-integration/src/channels/types.rs, src/agent/react/mod.rs]
    evidence_refs: [evidence.agent-context-execution, evidence.provider-protocol-quality]
  channel-reset-generation:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-integration/src/channels/session.rs, echo-integration/src/channels/types.rs, echo-integration/src/channels/channels/mod.rs]
    evidence_refs: [evidence.provider-protocol-quality]
---

# Protocol Stream、Cancel、Reset 与 Close 生命周期审计

## 审查范围

审查 A2A stream setup/cancel/drop/error、ACP/Headless/A2A/Channel Agent close、ACP terminal sink 与 Channel reset generation。

## 已检查故障假设

验证惰性 stream 未 poll/drop 是否泄漏 token，cancel 是否中断 stalled Agent，adapter close 是否 await Agent settlement，reset 后旧代输出是否可被 fencing。

## 实际实现路径与证据

A2A 在返回惰性 stream 前写 task/token，未 poll drop、setup/event error 和 disconnect 都无 RAII cleanup；cancel 只在 next event 后检查。ACP 已正确停止接纳、取消/等待 run并 await Agent close，应从 generic close Finding 排除。Headless不close，A2A/Channel无 Agent close contract，ReactAgent Drop cleanup未等待。Channel reset立即发布新 handler但允许旧 stream继续，OutboundMessage无 incarnation，旧代迟到回复不能拒绝。

## 问题记录

确认 A2A cleanup、adapter close 与 terminal commit order；新增 Channel stale-generation delivery，A2A task ID竞态复用 state Audit Finding。

## 残余风险

Channel generation activity count本身有较强测试，问题集中在输出 fencing和异步 close owner；reset 语义需 semantic-decide。

## 未检查项

未运行 A2A disconnect/重复 ID/ACP projector failure，未审查 HTTP graceful shutdown和全路径 backpressure。
