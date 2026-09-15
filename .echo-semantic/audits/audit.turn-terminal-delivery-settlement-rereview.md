---
schema_version: 1
id: audit.turn-terminal-delivery-settlement-rereview
kind: audit
boundary_ref: boundary.observation-persistence-delivery
lens: state_authority
freshness: examined
revision: cba8e08f3e3f0ccf1d4df3a22be11589f63b2ecd
finding_refs: [finding.turn-terminal-commit-projection-order]
challenges:
  execution-delivery-non-overwrite:
    revision: cba8e08f3e3f0ccf1d4df3a22be11589f63b2ecd
    source_refs: [echo-orchestration/src/runtime/turn_driver.rs, src/acp/runtime.rs, src/acp/adapter.rs]
    evidence_refs: [evidence.turn-terminal-delivery-settlement-repair, evidence.turn-terminal-delivery-settlement-verification]
  durable-recovery-settlement:
    revision: cba8e08f3e3f0ccf1d4df3a22be11589f63b2ecd
    source_refs: [echo-sdk-host/src/core_profile/persistence.rs, echo-sdk-host/src/core_profile/state.rs, echo-sdk-protocol/src/methods.rs]
    evidence_refs: [evidence.turn-terminal-delivery-settlement-repair, evidence.turn-terminal-delivery-settlement-verification]
---

# Turn execution 与 delivery 结算独立复审

## 审查范围

独立reviewer读取完整#108差异、Design、Plan、ADR 0046，以及driver、ACP、Headless、Eval、
SDK Host persistence/recovery和protocol消费者；其它并行差异被排除。

## 已检查故障假设

检查terminal sink失败覆盖producer终态、非terminal失败未取消、ACP把投影失败伪装成
`EndTurn`、重复receipt writer、非Completed记录携带final identity，以及Journal截断后恢复
仍接受较高receipt watermark。

## 实际实现路径与证据

前两轮review发现重复writer、真实Journal未复核、非法final fields与跨层测试不足等问题；
修复后第三次review确认execution和delivery独立归约、标准Prompt单写、Journal/index/receipt
fail-closed一致性，以及真实消费者测试均已闭合，最终结论pass。

## 问题记录

第三次review在最终快照上返回Critical 0、Important 0、Minor 0。

## 残余风险

Channel与A2A尚未统一进入同一Turn authority；这两个已知边界不由本Finding掩盖。

## 未检查项

未运行完整workspace合并门禁、真实远端provider或产品GUI路径。
