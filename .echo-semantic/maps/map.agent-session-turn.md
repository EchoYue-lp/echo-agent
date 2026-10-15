---
schema_version: 1
id: map.agent-session-turn
kind: capability_map
title: Agent、Session、Invocation 与 Turn
risk: high
observed_at: 554f2f50d3705d219519f306055a3d26d6a5211c
boundary_refs: [boundary.agent-session-turn]
behavior_refs: [behavior.channel-handler-invocation, behavior.agent-turn-lifecycle]
rule_refs: [rule.turn-terminal-authority, rule.context-persistence-separation]
evidence_refs: [evidence.subagent-physical-settlement-repair, evidence.agent-context-execution, evidence.high-risk-audit-frontier, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification, evidence.agent-adapter-close-settlement-repair, evidence.agent-adapter-close-settlement-verification, evidence.framework-concept-navigation]
finding_refs: [finding.subagent-physical-settlement-observation, finding.turn-driver-entry-coverage, finding.agent-adapter-close-settlement, finding.eval-timeout-settlement, finding.eval-trace-identity]
audit_refs: [audit.subagent-physical-settlement-focused-review, audit.channel-handler-invocation-rereview, audit.agent-session-turn.state-authority, audit.agent-adapter-close-settlement-rereview]
related_map_refs: [map.context-memory, map.task-subagent-workflow, map.observation-persistence-delivery, map.protocol-surfaces]
scenarios:
  channel-handler-forward-invocation:
    status: mapped
    source_refs: [echo-core/src/error.rs, echo-integration/src/channels/manager.rs, echo-integration/src/channels/types.rs, echo-integration/src/channels/session.rs, src/channels.rs, docs/adr/0085-channel-handler-invocation-ownership.md]
    behavior_refs: [behavior.channel-handler-invocation]
    rule_refs: [rule.channel-invocation-generation]
    finding_refs: [finding.channel-handler-invocation-settlement]
    evidence_refs: [evidence.channel-handler-forward-repair, evidence.channel-handler-forward-focused]
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
    finding_refs: [finding.turn-driver-entry-coverage]
    evidence_refs: [evidence.agent-adapter-close-settlement-repair, evidence.agent-adapter-close-settlement-verification]
    unknown: Raw ReactAgent API 是合理低层 contract，且close已闭合；Channel adapter仍绕过AgentTurnDriver
    next_step: 单独处理Channel terminal projection，不重建adapter close owner
  agent-revision:
    status: needs_review
    source_refs: [echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/workflow/graph.rs, echo-core/src/plugin/registry.rs]
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

ACP/Headless/SDK/Eval可投影不同结果并共享driven Turn terminal；Channel与直接Rust调用的
Turn-driver覆盖缺口已进入独立Finding，adapter close已闭合。

## 场景处置清单

Agent/Session与driven Turn已映射，Eval timeout、trace correlation与非A2A adapter close已闭合；
Channel/direct route、AgentRevision与factory同名保持needs_review。

## 未展开项

Context persistence、Task/Subagent 与 protocol adapter 由 related maps 展开。


本轮channel-handler-forward归原entry及原Session/Turn边界；新场景mapped表示owner、
生命周期、开放Finding和验证缺口有明确去向。23/80的新mutex修复与full23/exactmain
尚待，不能由历史22/79或本轮mapping代替。其它开放场景不在此关闭。


Channel forward当前本地三链闭合见audit.channel-handler-invocation-rereview；仅此场景具精确源码/工程收据复核，原其它needs_review不变。


## 已发布历史快照定位

本对象原 source:d0568a66d7ccb8955e15a0d6f7aaaf0799ae25c287264ded14a9f167a4e01fbe 的完整非语义源码与已发布 Git 554f2f50d3705d219519f306055a3d26d6a5211c 唯一摘要相同。全部本对象code/source引用逐blob核对当前仍相同，现只重绑定可恢复Git定位；原N审查范围、运行时间、状态与结论保持。此历史事实不覆盖D新增Database/RAG/Clinical三源码、其测试或消费者，更不冒作本轮门禁通过。

## B取消观察与close结算增量

原观察revision、needs_review与未闭路线不重标。本B增量由ADR0088及evidence.subagent-physical-settlement-repair的当前源码与独审承接：观察Unknown不是TurnReceipt/Task终态；原raw producer/Join、permit和资源不能由取消、logical terminal、socket或local task Drop代替。原Rule所述bounded reaper不能作为B当前强制释放策略。ACP close同receipt保Some原typed身份或无identity的None知识，同generation/repeat不重发；普通明确返回失败仍按原策略重试，真实close成功仍按原指针限定清理。完整gate/consumer/main与其它开放路线独立，不以本局部增量宣全域verified。

Headless便利观察返回经私有owner投影保原HeadlessRunHandle，内部stored result无owner环；原5值/格式不变，不把Unknown文本视为物理关闭。可能取消等待且需后续保原authority者用start_headless持handle，销毁最后parent不是停止凭证；新增Rust构造器/getter不自动接受为语言API。
