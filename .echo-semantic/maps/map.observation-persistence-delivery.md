---
schema_version: 1
id: map.observation-persistence-delivery
kind: capability_map
title: Observation、Persistence、Projection 与 Delivery
risk: high
observed_at: f1e9027246760661144786e9e35615cd46d580c6
boundary_refs: [boundary.observation-persistence-delivery]
behavior_refs: [behavior.observation-persistence]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation, evidence.high-risk-audit-frontier]
finding_refs: [finding.trace-effect-event-producers, finding.eval-trace-identity, finding.trace-audit-secret-boundary, finding.turn-terminal-commit-projection-order, finding.checkpoint-journal-binding, finding.diagnostic-persistence-failure-visibility, finding.hook-event-producer-contract, finding.in-memory-audit-successful-drop]
audit_refs: [audit.observation-persistence-delivery.state-authority, audit.observation-persistence-delivery.data-durability, audit.observation-persistence-delivery.contract-evidence]
related_map_refs: [map.agent-session-turn, map.context-memory, map.task-subagent-workflow, map.protocol-surfaces, map.eval-evolution]
scenarios:
  versioned-agent-subagent-events:
    status: mapped
    source_refs: [echo-core/src/agent/event_envelope.rs, src/agent/subagent/events.rs]
    behavior_refs: [behavior.observation-persistence]
    rule_refs: [rule.fact-projection-separation]
  journal-checkpoint-recovery:
    status: needs_review
    source_refs: [echo-state/src/journal/mod.rs, echo-state/src/journal/file.rs, echo-state/src/journal/segmented.rs]
    rule_refs: [rule.fact-projection-separation]
    evidence_refs: [evidence.persistence-observation]
    finding_refs: [finding.checkpoint-journal-binding]
    unknown: checkpoint 未绑定来源 Journal identity，合法异源 state 可通过序号检查
    next_step: repair checkpoint source/scope binding 并补异源恢复测试
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
    finding_refs: [finding.trace-effect-event-producers, finding.eval-trace-identity, finding.trace-audit-secret-boundary, finding.turn-terminal-commit-projection-order, finding.diagnostic-persistence-failure-visibility, finding.in-memory-audit-successful-drop]
    evidence_refs: [evidence.persistence-observation]
    unknown: trace producer/identity、terminal commit order 与各 audit backend 的 retention/redaction/failure visibility 未闭合
    next_step: 分别 repair identity/producer/commit order，并裁决诊断 backend failure policy
  complete-event-family-classification:
    status: needs_review
    source_refs: [echo-core/src/hooks/types.rs, echo-orchestration/src/tasks/events.rs, echo-orchestration/src/workflow/mod.rs, src/trace/mod.rs]
    finding_refs: [finding.trace-effect-event-producers, finding.hook-event-producer-contract, finding.workflow-entry-loop-drift]
    unknown: 全部 Agent/Task/Subagent/Workflow/Hook/Trace/Delivery events 的 durable、versioned、lossy、diagnostic 与 replay 属性尚未逐项反证
    next_step: producer matrix 已建立；逐 family 决定补生产路径或收窄 enum/docs，并增加 lag/terminal 合同测试
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
