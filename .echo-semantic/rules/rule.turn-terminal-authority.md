---
schema_version: 1
id: rule.turn-terminal-authority
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: time_lifecycle
focus: [state_authority, failure_concurrency, contract_evidence]
observed_at: 81e2756cee9127fa23a9bb1023bd56aa8f954964
behavior_refs: [behavior.agent-turn-lifecycle]
code_refs: [echo-orchestration/src/runtime/turn_driver.rs, echo-core/src/agent/event_envelope.rs, src/headless.rs, src/acp/runtime.rs, src/eval/runner.rs, src/channels.rs, src/agent/react/mod.rs, docs/adr/0009-tracked-input-receipts.md, docs/adr/0010-canonical-turn-receipt-accounting.md, docs/adr/0037-eval-timeout-turn-settlement.md]
evidence_refs: [evidence.agent-context-execution, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification]
finding_refs: [finding.turn-driver-entry-coverage, finding.eval-timeout-settlement]
---

# Turn 终态权威

## 不变量或唯一权威

`AgentTurnDriver` 提交一个 monotonic `EventEnvelope` 序列，`TurnReceipt` 是一次 driven invocation 的唯一通用终态与计量摘要。

## 适用行为

适用于Headless、ACP、Eval、经ACP的SDK和其它显式通过driver执行的Turn，以及tracked steer input；不自动覆盖raw ReactAgent或Channel调用。

## 当前实现

Driver在sink前记录事实，区分Completed/Cancelled/Failed，并保存final answer、usage、compaction、last sequence和elapsed time。ReactAgent managed stream在producer task settled后才释放terminal；提前drop才走bounded reaper。Eval deadline只发出cancel request，必须继续等待同一driver future取得receipt，或在共享bounded grace后显式标记未settled。

## 期望行为

EOF、renderer、trace、cancel request或产品observer不得自行推断成功；失败投递不能保留成功receipt字段。

## 证据

Turn driver 源码、tracked input/receipt ADR 与集成测试覆盖主要终态和 sink failure 场景。

## 裁决记录

ADR 0010 已接受；Channel/direct 是否应进入 driver 尚待 route audit，故本 Rule 保持 needs_review。
