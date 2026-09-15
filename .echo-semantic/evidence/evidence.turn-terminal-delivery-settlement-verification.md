---
schema_version: 1
id: evidence.turn-terminal-delivery-settlement-verification
kind: evidence
observed_at: cba8e08f3e3f0ccf1d4df3a22be11589f63b2ecd
source_refs:
  - echo-orchestration/src/runtime/turn_driver.rs
  - tests/acp_agent_adapter.rs
  - tests/agent_handle_turn_driver.rs
  - echo-sdk-host/tests/core_profile_e2e.rs
  - echo-sdk-protocol/tests/core_rpc_contract.rs
  - src/headless.rs
  - src/eval/runner.rs
supports: [behavior.agent-turn-lifecycle, behavior.observation-persistence, rule.turn-terminal-authority, rule.fact-projection-separation]
limitations:
  - 完整workspace门禁与远端CI留到汇总MR前执行
  - 通用Closed由可关闭EventSink反例覆盖，ACP transport failure按设计映射为Failed或取消
---

# Turn execution 与 delivery 结算验证证据

## 支持的结论

定向验证覆盖driver的Delivered、NotAttempted、Closed、terminal/pre-terminal Failed、缺失
terminal和错误envelope；ACP覆盖projection/observer失败与`EndTurn`门禁；Headless、Eval、
AgentHandle和SDK Host覆盖真实消费、持久化、历史恢复、Journal截断/缺失与watermark不一致。
最新SDK protocol合同22项全部通过，包含非Completed final fields拒绝以及delivery failure
无损wire round-trip。

## 来源与范围

第三次独立review锁定HEAD `b97838d2`加最终#108未提交快照；实现随后原样提交为
`cba8e08f3e3f0ccf1d4df3a22be11589f63b2ecd`。Reviewer确认Critical、Important和Minor
均为0，`git diff --check`通过。

## 已知缺口

本证据是Finding级定向验证；全workspace、all-feature、no-default、feature matrix与SDK
生成合同由最终MR门禁统一执行。
