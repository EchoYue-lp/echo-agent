---
schema_version: 1
id: evidence.turn-terminal-delivery-settlement-repair
kind: evidence
observed_at: cba8e08f3e3f0ccf1d4df3a22be11589f63b2ecd
source_refs:
  - echo-orchestration/src/runtime/turn_driver.rs
  - src/acp/runtime.rs
  - src/acp/adapter.rs
  - echo-sdk-host/src/core_profile/persistence.rs
  - echo-sdk-host/src/core_profile/state.rs
  - echo-sdk-protocol/src/methods.rs
  - docs/adr/0046-turn-execution-delivery-settlement.md
supports: [behavior.agent-turn-lifecycle, behavior.observation-persistence, rule.turn-terminal-authority, rule.fact-projection-separation]
limitations:
  - Channel与raw ReactAgent入口是否统一进入TurnDriver仍由finding.turn-driver-entry-coverage追踪
  - A2A自行归约执行终态的问题仍由finding.a2a-terminal-authority追踪
---

# Turn execution 与 delivery 结算修复证据

## 支持的结论

`TurnReceipt`现在分别携带producer-owned execution outcome和typed delivery outcome。
terminal event已经确定Completed、Cancelled或Failed后，sink关闭或失败不会改写执行终态、
final answer、message identity、usage或compaction；terminal之前的sink失败仍取消producer并
形成Failed execution与Failed delivery。

ACP只在`Completed + Delivered`时产生`EndTurn`。标准Prompt只有
`persist_run_settled`写入完整receipt；恢复时Journal、index和receipt的identity、终态、
final fields与watermark必须一致，否则fail closed。旧wire记录缺少delivery字段时保留为
`legacy_unknown`，不推断成功。

## 来源与范围

实现固定于commit `cba8e08f3e3f0ccf1d4df3a22be11589f63b2ecd`，复用既有
`AgentTurnDriver`、ACP Ledger/Journal和`RunReceiptWire`，没有引入第二执行状态机或产品层
projection到framework。

## 已知缺口

本证据不关闭Channel、A2A、transcript outbox或异步delivery retry语义；这些继续由独立
Finding追踪。
