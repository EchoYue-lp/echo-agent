---
schema_version: 1
id: audit.observation-persistence-delivery.state-authority
kind: audit
boundary_ref: boundary.observation-persistence-delivery
lens: state_authority
freshness: examined
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.eval-trace-identity, finding.trace-effect-event-producers, finding.turn-terminal-commit-projection-order]
challenges:
  event-family-authority:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-core/src/agent/event_envelope.rs, echo-orchestration/src/tasks/events.rs, src/agent/subagent/events.rs, echo-orchestration/src/workflow/mod.rs, echo-state/src/journal/mod.rs, echo-state/src/delivery.rs]
    evidence_refs: [evidence.persistence-observation]
  terminal-commit-and-projection:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/agent/react/run/phases/finalize.rs, echo-orchestration/src/runtime/turn_driver.rs, src/acp/runtime.rs, src/trace/mod.rs]
    evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation]
  eval-trace-identity:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/eval/runner.rs, src/agent/react/mod.rs, src/agent/react/run/stream_channel.rs]
    evidence_refs: [evidence.provider-protocol-quality, evidence.persistence-observation]
---

# Observation、Projection 与终态权威审计

## 审查范围

审查 Agent/Task/Subagent/Workflow/Hook/Trace/Delivery event family、Turn terminal commit、ACP projection、RunStore 与 Eval trace lookup。

## 已检查故障假设

验证 live/projection/trace 是否被误作 durable fact，多个终态载体是否在 sink failure 时冲突，以及 Eval 是否以 product run ID 猜测 trace ID。

## 实际实现路径与证据

Raw AgentEvent 是 live payload；EventEnvelope 只在具体 sink 接 Journal 后 durable。TaskEvent 是 lossy UI progress，无 terminal；Subagent envelope 提供进程内 bounded replay；WorkflowEvent 是临时 stream；Hook/Audit event 不驱动领域恢复。EventJournal 与 DeliveryLedger 只对显式接入领域拥有 durable facts。ReactAgent 在 FinalAnswer 投递前先写 checkpoint/transcript 并完成 trace，而 TurnDriver sink failure 可返回 Failed；ACP 也可能先 commit ledger 再因 projector/observer error 返回 Failed。Eval 用 product run ID 查询实际随机 child trace ID，确认 identity mismatch。

## 问题记录

确认 Eval trace identity 与缺失 trace producers；新增 terminal commit/projection order Finding。`rule.fact-projection-separation` 继续保持 needs_review，不建立 universal EventStore。

## 残余风险

需要明确唯一 terminal commit point，并把 projection/observer failure 建模为独立 delivery failure，不能反向改写已提交业务终态。

## 未检查项

未展开 A2A 独立终态、所有 backend corruption/retention 或应用 GUI/feed addressing。
