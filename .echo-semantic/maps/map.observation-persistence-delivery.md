---
schema_version: 1
id: map.observation-persistence-delivery
kind: capability_map
title: Observation、Persistence、Projection 与 Delivery
risk: high
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
boundary_refs: [boundary.observation-persistence-delivery]
behavior_refs: [behavior.observation-persistence]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation]
finding_refs: [finding.trace-effect-event-producers, finding.eval-trace-identity, finding.trace-audit-secret-boundary]
audit_refs: []
related_map_refs: [map.agent-session-turn, map.context-memory, map.task-subagent-workflow, map.protocol-surfaces, map.eval-evolution]
scenarios:
  versioned-agent-subagent-events:
    status: mapped
    source_refs: [echo-core/src/agent/event_envelope.rs, src/agent/subagent/events.rs]
    behavior_refs: [behavior.observation-persistence]
    rule_refs: [rule.fact-projection-separation]
  journal-checkpoint-recovery:
    status: mapped
    source_refs: [echo-state/src/journal/mod.rs, echo-state/src/journal/file.rs, echo-state/src/journal/segmented.rs]
    rule_refs: [rule.fact-projection-separation]
    evidence_refs: [evidence.persistence-observation]
  store-role-separation:
    status: mapped
    source_refs: [src/state/mod.rs, echo-core/src/memory/conversation.rs, echo-core/src/memory/store.rs, src/trace/mod.rs]
    behavior_refs: [behavior.observation-persistence]
    rule_refs: [rule.fact-projection-separation]
  delivery-ledger:
    status: mapped
    source_refs: [echo-state/src/delivery.rs, docs/adr/0019-typed-delivery-ledger-api.md]
    behavior_refs: [behavior.observation-persistence]
    evidence_refs: [evidence.persistence-observation]
  trace-event-and-identity-coverage:
    status: needs_review
    source_refs: [src/trace/mod.rs, src/eval/runner.rs, src/agent/react/mod.rs, src/agent/react/run/pipeline.rs, echo-state/src/audit/memory.rs]
    finding_refs: [finding.trace-effect-event-producers, finding.eval-trace-identity, finding.trace-audit-secret-boundary]
    evidence_refs: [evidence.persistence-observation]
    unknown: trace producer/identity 与各 audit backend 的原始输入 retention/redaction contract 未闭合
    next_step: observation audit 逐 producer/backend 验证 identity、secret boundary 和 failure visibility
  complete-event-family-classification:
    status: needs_review
    source_refs: [echo-core/src/hooks/types.rs, echo-orchestration/src/tasks/events.rs, echo-orchestration/src/workflow/mod.rs, src/trace/mod.rs]
    unknown: 全部 Agent/Task/Subagent/Workflow/Hook/Trace/Delivery events 的 durable、versioned、lossy、diagnostic 与 replay 属性尚未逐项反证
    next_step: high-risk observation audit 逐 family 验证 producer、ordering、retention、terminal 和 consumer
---

# Observation、Persistence、Projection 与 Delivery

## 能力范围

覆盖 EventEnvelope、各 event family、Journal/checkpoint、RuntimeState/Conversation/Memory/Run stores、projection/replay/retention 与 DeliveryLedger。

## 入口与输出

领域事件、checkpoint save、store CRUD、delivery transition 与 trace record 进入；输出 durable facts、rebuildable state、live feed 和诊断报告。

## 行为关系

每个数据域指定自己的 Store/Journal；命名相似不代表同层抽象，Trace 和 UI projection 不驱动业务恢复。

## 状态与数据流

EventEnvelope 保持 identity/sequence，Journal 先 commit 后 reduce，checkpoint 绑定 applied sequence，DeliveryLedger 归约 typed lifecycle。

## 策略来源与优先级

领域 Rule/ADR 决定 fact authority；retention/config 决定 bounded history；消费者只在明确合同下 ack/replay/project。

## 生命周期与失败路径

Append/reconcile/replay/checkpoint/recover/prune，stream lag/gap/EOF，store corruption/partial write，delivery claim/effect/drain/settle。

## 权限与敏感信息

Trace/audit/tool output 的 secret redaction 与 retention 必须由具体 backend 明确；当前 in-memory trace/audit 可保留原始输入，不能作全局脱敏保证。

## 用户侧投影

Conversation history、Task progress、ACP updates 和 diagnostics 是从事实派生的 bounded view。

## 场景处置清单

核心 authority 已映射；trace producer/identity 有 Findings，完整 event family 分类保持 needs_review。

## 未展开项

应用 UI/feed retention 与产品 addressing 不进入 framework baseline。
