---
schema_version: 1
id: rule.turn-terminal-authority
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: time_lifecycle
focus: [state_authority, failure_concurrency, contract_evidence]
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
behavior_refs: [behavior.agent-turn-lifecycle]
code_refs: [echo-orchestration/src/runtime/turn_driver.rs, echo-core/src/agent/event_envelope.rs, src/headless.rs, src/acp/runtime.rs, src/channels.rs, src/agent/react/mod.rs, docs/adr/0009-tracked-input-receipts.md, docs/adr/0010-canonical-turn-receipt-accounting.md]
evidence_refs: [evidence.agent-context-execution]
finding_refs: [finding.turn-driver-entry-coverage]
---

# Turn 终态权威

## 不变量或唯一权威

`AgentTurnDriver` 提交一个 monotonic `EventEnvelope` 序列，`TurnReceipt` 是一次 driven invocation 的唯一通用终态与计量摘要。

## 适用行为

适用于 Headless、ACP、经 ACP 的 SDK 和其它显式通过 driver 执行的 Turn，以及 tracked steer input；不自动覆盖 raw ReactAgent 或 Channel 调用。

## 当前实现

Driver 在 sink 前记录事实，区分 Completed/Cancelled/Failed，并保存 final answer、usage、compaction、last sequence 和 elapsed time。

## 期望行为

EOF、renderer、trace 或产品 observer 不得自行推断成功；失败投递不能保留成功 receipt 字段。

## 证据

Turn driver 源码、tracked input/receipt ADR 与集成测试覆盖主要终态和 sink failure 场景。

## 裁决记录

ADR 0010 已接受；Channel/direct 是否应进入 driver 尚待 route audit，故本 Rule 保持 needs_review。
