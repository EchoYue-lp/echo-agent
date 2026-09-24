---
schema_version: 1
id: rule.turn-terminal-authority
kind: rule
status: needs_review
expectation: human_confirmed
risk: high
primary_focus: time_lifecycle
focus: [state_authority, failure_concurrency, contract_evidence]
observed_at: 733d352fc719f922b21bab1cd46206139564367f
behavior_refs: [behavior.agent-turn-lifecycle]
code_refs: [echo-orchestration/src/runtime/turn_driver.rs, echo-core/src/agent/event_envelope.rs, src/headless.rs, src/acp/runtime.rs, src/eval/runner.rs, src/channels.rs, src/agent/react/mod.rs, src/agent/react/lifecycle.rs, docs/adr/0009-tracked-input-receipts.md, docs/adr/0010-canonical-turn-receipt-accounting.md, docs/adr/0037-eval-timeout-turn-settlement.md, docs/adr/0046-turn-execution-delivery-settlement.md, docs/adr/0066-agent-adapter-close-ownership.md]
evidence_refs: [evidence.agent-context-execution, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification, evidence.turn-terminal-delivery-settlement-repair, evidence.turn-terminal-delivery-settlement-verification, evidence.agent-adapter-close-settlement-repair, evidence.agent-adapter-close-settlement-verification]
finding_refs: [finding.turn-driver-entry-coverage, finding.eval-timeout-settlement, finding.turn-terminal-commit-projection-order]
---

# Turn 终态权威

## 不变量或唯一权威

`AgentTurnDriver` 提交一个 monotonic `EventEnvelope` 序列，`TurnReceipt` 是一次 driven invocation 的唯一通用终态与计量摘要。

## 适用行为

适用于Headless、ACP、Eval、Channel、经ACP的SDK和其它显式通过driver执行的Turn，以及tracked steer input；不自动覆盖raw ReactAgent或A2A协议Task。

## 当前实现

Driver先由producer terminal确定Completed/Cancelled/Failed，再独立结算sink的Delivered、Closed或Failed；delivery不得覆盖已提交的执行终态和final facts。ReactAgent managed stream在producer task settled后才释放terminal；提前drop才走bounded reaper。Eval deadline只发出cancel request，必须继续等待同一driver future取得receipt，或在共享bounded grace后显式标记未settled。
stream start在创建producer前返回typed cancellation时，Driver仍发布Cancelled receipt，不包装成Failed。
Adapter resource close由各自owner等待`Agent::close`；React close authority只fence/cancel/wait既有
Turn终态，不提交第二终态。它以child token隔离caller scope，并把preparation/producer异常Drop记录为
persistent close debt；debt阻断后续资源释放。close错误不反向改写同一TurnReceipt执行终态，也不能用新的adapter terminal替代它。

## 期望行为

EOF、renderer、trace、cancel request或产品observer不得自行推断执行成功；失败投递必须在delivery中显式可见，且不得反向改写producer-owned execution fields。

## 证据

Turn driver源码、tracked input/receipt ADR、execution/delivery修复与跨层集成测试覆盖主要终态、sink failure及持久化恢复场景。

## 裁决记录

ADR 0010 已接受；Channel/direct 是否应进入 driver 尚待 route audit，故本 Rule 保持 needs_review。
