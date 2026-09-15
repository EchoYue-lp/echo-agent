---
schema_version: 1
id: audit.sse-eof-framing-acceptance-rereview
kind: audit
boundary_ref: boundary.llm-provider-runtime
lens: failure_concurrency
freshness: examined
revision: e842d87bb787fb0b1fd39afbc04f834df07aec84
finding_refs: [finding.sse-eof-framing-acceptance]
challenges:
  delimiterless-eof-rejection:
    revision: e842d87bb787fb0b1fd39afbc04f834df07aec84
    source_refs: [echo-integration/src/providers/client.rs]
    evidence_refs: [evidence.sse-eof-framing-acceptance-repair, evidence.sse-eof-framing-acceptance-verification]
  provider-entry-parity:
    revision: e842d87bb787fb0b1fd39afbc04f834df07aec84
    source_refs: [echo-integration/src/providers/openai.rs, echo-integration/src/providers/responses.rs, echo-integration/src/providers/anthropic.rs]
    evidence_refs: [evidence.sse-eof-framing-acceptance-verification]
---

# SSE EOF framing独立复审

## 审查范围

独立reviewer检查共享decoder、三provider真实入口、合法framing、截断输入和取消顺序，
排除其它并行provider与SDK差异。

## 已检查故障假设

检查delimiterless完整JSON被误接受、纯空白被当正常EOF、CRLF事件回归、拆分UTF-8误判，
以及EOF检查覆盖调用方取消或provider semantic terminal。

## 实际实现路径与证据

`finish`只接受空buffer；所有残余返回`InvalidResponse`。合法事件在`next_event`消费后留下
空buffer，取消在EOF检查前保持原typed结果。三provider共享这一条transport路径。

## 问题记录

最终review结论pass，Critical 0、Important 0、Minor 0。

## 残余风险

Provider stream semantic terminal parity由#78继续追踪。

## 未检查项

未运行真实公网provider、完整workspace门禁或远端CI。
