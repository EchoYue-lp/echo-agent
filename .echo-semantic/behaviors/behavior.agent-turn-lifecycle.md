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
observed_at: source:5806bd920140828b759fabe868fc1c4af1f26009ed4bf9112b73385cf2ce764e
code_refs: [echo-core/src/agent/mod.rs, echo-core/src/agent/event_envelope.rs, echo-core/src/tools/mod.rs, src/agent/react/mod.rs, src/agent/react/run/stream_channel.rs, src/agent/handle.rs, echo-orchestration/src/runtime/turn_driver.rs, src/acp/session.rs, src/acp/runtime.rs, src/acp/adapter.rs, src/headless.rs, src/eval/runner.rs, src/channels.rs, echo-integration/src/channels/manager.rs, echo-integration/src/channels/types.rs, echo-integration/src/channels/session.rs, docs/adr/0037-eval-timeout-turn-settlement.md, docs/adr/0038-eval-trace-correlation-identity.md, docs/adr/0046-turn-execution-delivery-settlement.md, docs/adr/0066-agent-adapter-close-ownership.md]
rule_refs: [rule.turn-terminal-authority, rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification, evidence.turn-terminal-delivery-settlement-repair, evidence.turn-terminal-delivery-settlement-verification, evidence.agent-adapter-close-settlement-repair, evidence.agent-adapter-close-settlement-verification]
finding_refs: [finding.turn-driver-entry-coverage, finding.eval-timeout-settlement, finding.eval-trace-identity, finding.turn-terminal-commit-projection-order]
---

# Agent、Session 与 Turn 生命周期

## 重要承诺

一次由 `AgentTurnDriver` 接纳的有限 invocation 只有一个事件序列和一个 `TurnReceipt` 终态；Session/Conversation/runtime incarnation 是限定身份，不合并为新的通用状态机。

## 当前行为

`ReactAgent`实现原始Agent调用、生成真实trace Run，并在managed stream中等待自有producer settled后才释放terminal；`AgentTurnDriver`接纳输入、提交envelope、归约usage/final output，并把producer execution outcome与sink delivery outcome写入同一receipt。ACP、Headless、Channel、经ACP的SDK与Eval使用driver；A2A与raw Rust execute/chat保持现有边界。ACP adapter在poll连接future前同步交出同一registry/services的close handle，未保留时拒绝创建Agent；registry保留未结算Run和Agent。Headless一次性await close，Channel保留长期owner并可重试地关闭资源；ReactAgent Drop不再假称异步cleanup已结算。A2A不在本轮修复范围。

## 期望行为

EOF、取消和close不得被投影为成功；sink失败必须作为delivery failure可见，但已观察到的producer terminal不得被它覆盖。应用可持久化或渲染receipt，但不能重新判断通用执行终态。

## 触发、结果与副作用

ACP prompt、Headless prompt、Eval case和经ACP的SDK call触发driven Turn；direct execute/chat/stream与Channel message触发原始Agent execution。结果可能包含event stream、receipt、EvalResult、checkpoint/transcript写入以及工具或Subagent effect。

## 失败、重试与恢复

输入需区分accepted、drained与turn-settled；取消传播、Eval timeout、Session close和runtime restore必须等待receipt或明确界定未结算effect。Adapter关闭失败或被调用方取消时，不把transport EOF或drop当成功；仍存活的registry/server/manager/session owner保留资源以供重试。

## 证据

Agent/ReactAgent、Turn driver、EvalRunner、Session registries、tracked receipt/timeout/trace correlation ADR与integration tests覆盖driven路径；Channel/direct差异由Finding保留。

## 裁决记录

ADR 0009/0010 确认 tracked input 与 TurnReceipt 权威；用户要求明确 Agent/Session/Turn 关系而不新增裸 `Run` 状态机。
