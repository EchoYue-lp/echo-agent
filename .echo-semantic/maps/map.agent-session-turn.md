---
schema_version: 1
id: map.agent-session-turn
kind: capability_map
title: Agent、Session、Invocation 与 Turn
risk: high
observed_at: source:66a74859cd586d80d2ad791b3a2369b31e7bcff60f6afb6a4edf3575a29a7778
boundary_refs: [boundary.agent-session-turn]
behavior_refs: [behavior.agent-turn-lifecycle]
rule_refs: [rule.turn-terminal-authority, rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution, evidence.high-risk-audit-frontier, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification, evidence.framework-concept-navigation]
finding_refs: [finding.turn-driver-entry-coverage, finding.agent-adapter-close-settlement, finding.eval-timeout-settlement, finding.eval-trace-identity]
audit_refs: [audit.agent-session-turn.state-authority]
related_map_refs: [map.context-memory, map.task-subagent-workflow, map.observation-persistence-delivery, map.protocol-surfaces]
scenarios:
  agent-definition-and-instance:
    status: mapped
    source_refs: [echo-core/src/agent/mod.rs, src/agent/react/mod.rs, src/agent/handle.rs]
    behavior_refs: [behavior.agent-turn-lifecycle]
    evidence_refs: [evidence.agent-context-execution]
  session-and-conversation-identities:
    status: mapped
    source_refs: [src/acp/session.rs, echo-integration/src/channels/session.rs, src/state/mod.rs]
    rule_refs: [rule.context-persistence-separation]
    evidence_refs: [evidence.agent-context-execution]
  driven-turn-admission-and-terminal:
    status: mapped
    source_refs: [echo-orchestration/src/runtime/turn_driver.rs, echo-core/src/agent/event_envelope.rs, echo-core/src/tools/mod.rs, src/headless.rs, src/acp/runtime.rs, src/eval/runner.rs, docs/adr/0037-eval-timeout-turn-settlement.md, docs/adr/0038-eval-trace-correlation-identity.md]
    behavior_refs: [behavior.agent-turn-lifecycle]
    rule_refs: [rule.turn-terminal-authority]
    finding_refs: [finding.eval-timeout-settlement, finding.eval-trace-identity]
    evidence_refs: [evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification]
  direct-and-channel-execution:
    status: needs_review
    source_refs: [src/agent/react/mod.rs, src/channels.rs]
    finding_refs: [finding.turn-driver-entry-coverage, finding.agent-adapter-close-settlement]
    unknown: Raw ReactAgent API 是合理低层 contract；Channel adapter 绕过 AgentTurnDriver，且 adapter close/Agent resource settlement 尚无统一合同
    next_step: 对 Channel terminal projection 与各 adapter awaited close 分别形成 repair/decision
  agent-revision:
    status: needs_review
    source_refs: [echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/workflow/graph.rs, echo-core/src/plugin/registry.rs, echo-sdk-host/src/core_profile/state.rs]
    unknown: 仓库不存在通用 AgentRevision；Task/Workflow/Plugin/schema/generation revisions 是否需要共同 glossary 而非新 aggregate
    next_step: 在 architecture audit 中确认限定术语并禁止新增裸 Revision authority
  agent-factory-naming:
    status: needs_review
    source_refs: [echo-core/src/agent/factory.rs, src/agent/subagent/registry.rs]
    unknown: 两个同名 AgentFactory 服务不同生命周期，是否需要限定命名以减少 API 歧义
    next_step: 审查调用方和兼容影响，默认保留两个合理 public capability
---

# Agent、Session、Invocation 与 Turn

## 能力范围

覆盖 Agent 构造/实例、ACP/Channel Session、Conversation/runtime incarnation、Invocation/Turn 与 terminal receipt。

## 入口与输出

Execute/chat/stream、Headless、ACP prompt、Eval case、Channel message 和 SDK call 进入 Agent；输出 event stream、TurnReceipt、EvalResult 与外部 effect。

## 行为关系

Agent 实现原始执行，Session 管入口作用域，Context 保存模型状态；Headless、ACP、经ACP的SDK与Eval等driven invocation由Turn driver统一终态。

## 状态与数据流

Session ID、conversation ID、runtime state ID、product run ID、invocation correlation与trace ID均为限定identity；TurnReceipt不替代持久Task、trace Run或transcript。

## 策略来源与优先级

AgentConfig/InvocationContext、Session adapter、ADR 0001/0005/0006/0009/0010 共同决定 scope 与生命周期。

## 生命周期与失败路径

Create/resolve Agent、admit/accept/drain Turn、cancel/fail/complete、bounded timeout settlement、close Session/Agent；cancel request与EOF都不构成成功。

## 权限与敏感信息

Agent 自动 effect 交给 permission map；Session scope 与 secret-bearing config 不进入 event/trace 明文。

## 用户侧投影

ACP/Headless/SDK/Eval可投影不同结果并共享driven Turn terminal；Channel与直接Rust调用的覆盖缺口已进入Finding。

## 场景处置清单

Agent/Session与driven Turn已映射，Eval timeout与trace correlation已闭合；Channel/direct route、AgentRevision与factory同名保持needs_review。

## 未展开项

Context persistence、Task/Subagent 与 protocol adapter 由 related maps 展开。
