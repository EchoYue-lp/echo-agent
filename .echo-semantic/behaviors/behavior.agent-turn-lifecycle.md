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
observed_at: f1e9027246760661144786e9e35615cd46d580c6
code_refs: [echo-core/src/agent/mod.rs, src/agent/react/mod.rs, src/agent/handle.rs, echo-orchestration/src/runtime/turn_driver.rs, src/acp/session.rs, src/acp/runtime.rs, src/headless.rs, src/channels.rs, echo-integration/src/channels/session.rs]
rule_refs: [rule.turn-terminal-authority, rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution]
finding_refs: [finding.turn-driver-entry-coverage]
---

# Agent、Session 与 Turn 生命周期

## 重要承诺

一次由 `AgentTurnDriver` 接纳的有限 invocation 只有一个事件序列和一个 `TurnReceipt` 终态；Session/Conversation/runtime incarnation 是限定身份，不合并为新的通用状态机。

## 当前行为

`ReactAgent` 实现原始 Agent 调用；`AgentTurnDriver` 接纳输入、提交 envelope、归约 usage/final output 并生成 Completed/Cancelled/Failed receipt。ACP、Headless 与经 ACP 的 SDK 使用 driver；Channel 和直接 Rust execute/chat 当前绕过 driver。

## 期望行为

EOF、sink 失败、取消和 close 不得被投影为成功；应用可持久化或渲染 receipt，但不能重新判断通用终态。

## 触发、结果与副作用

ACP prompt、Headless prompt 和经 ACP 的 SDK call 触发 driven Turn；direct execute/chat/stream 与 Channel message 触发原始 Agent execution。结果可能包含 event stream、receipt、checkpoint/transcript 写入以及工具或 Subagent effect。

## 失败、重试与恢复

输入需区分 accepted、drained 与 turn-settled；取消传播、Session close 和 runtime restore 必须等待或明确界定未结算 effect。

## 证据

Agent/ReactAgent、Turn driver、Session registries、tracked receipt ADR 与 integration tests 覆盖 driven 路径；Channel/direct 差异由 Finding 保留。

## 裁决记录

ADR 0009/0010 确认 tracked input 与 TurnReceipt 权威；用户要求明确 Agent/Session/Turn 关系而不新增裸 `Run` 状态机。
