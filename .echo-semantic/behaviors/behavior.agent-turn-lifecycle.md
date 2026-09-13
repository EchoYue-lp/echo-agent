---
schema_version: 1
id: behavior.agent-turn-lifecycle
kind: behavior
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: time_lifecycle
focus: [state_authority, failure_concurrency, result_side_effect, contract_evidence]
boundary: boundary.agent-session-turn
observed_at: source:0ff44ba1010dfd579acdd80c3f9d369c3d87f1dcbe05ba8840f5de7c96be4e61
code_refs: [echo-core/src/agent/mod.rs, src/agent/react/mod.rs, src/agent/react/run/stream_channel.rs, src/agent/handle.rs, echo-orchestration/src/runtime/turn_driver.rs, src/acp/session.rs, src/acp/runtime.rs, src/headless.rs, src/eval/runner.rs, src/channels.rs, echo-integration/src/channels/session.rs, docs/adr/0037-eval-timeout-turn-settlement.md]
rule_refs: [rule.turn-terminal-authority, rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification]
finding_refs: [finding.turn-driver-entry-coverage, finding.eval-timeout-settlement]
---

# Agent、Session 与 Turn 生命周期

## 重要承诺

一次由 `AgentTurnDriver` 接纳的有限 invocation 只有一个事件序列和一个 `TurnReceipt` 终态；Session/Conversation/runtime incarnation 是限定身份，不合并为新的通用状态机。

## 当前行为

`ReactAgent`实现原始Agent调用，并在managed stream中等待自有producer settled后才释放terminal；`AgentTurnDriver`接纳输入、提交envelope、归约usage/final output并生成Completed/Cancelled/Failed receipt。ACP、Headless、经ACP的SDK与Eval使用driver；Eval deadline后在同一个drive future上等待共享bounded grace。Channel和直接Rust execute/chat当前绕过driver。

## 期望行为

EOF、sink 失败、取消和 close 不得被投影为成功；应用可持久化或渲染 receipt，但不能重新判断通用终态。

## 触发、结果与副作用

ACP prompt、Headless prompt、Eval case和经ACP的SDK call触发driven Turn；direct execute/chat/stream与Channel message触发原始Agent execution。结果可能包含event stream、receipt、EvalResult、checkpoint/transcript写入以及工具或Subagent effect。

## 失败、重试与恢复

输入需区分accepted、drained与turn-settled；取消传播、Eval timeout、Session close和runtime restore必须等待receipt或明确界定未结算effect。

## 证据

Agent/ReactAgent、Turn driver、EvalRunner、Session registries、tracked receipt/timeout ADR与integration tests覆盖driven路径；Channel/direct差异由Finding保留。

## 裁决记录

ADR 0009/0010 确认 tracked input 与 TurnReceipt 权威；用户要求明确 Agent/Session/Turn 关系而不新增裸 `Run` 状态机。
