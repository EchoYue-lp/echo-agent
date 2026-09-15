---
schema_version: 1
id: finding.sse-eof-framing-acceptance
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [contract_evidence, time_lifecycle]
boundary_ref: boundary.llm-provider-runtime
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.sse-eof-framing-acceptance-repair, evidence.sse-eof-framing-acceptance-verification]
audit_refs: [audit.llm-provider-runtime.failure-concurrency, audit.sse-eof-framing-acceptance-rereview]
decision_refs: []
repair_evidence_refs: [evidence.sse-eof-framing-acceptance-repair]
verification_evidence_refs: [evidence.sse-eof-framing-acceptance-verification]
rereview_audit_refs: [audit.sse-eof-framing-acceptance-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# SSE EOF 接受缺事件边界的剩余 JSON

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/95

## 问题

SseDecoder::finish 把没有空行分隔符的剩余 buffer 当事件返回，只要 data JSON 完整就 yield，而不是 truncated-event error。

## 触发条件与影响

连接在事件 framing 完成前断开时，部分 provider payload 可被当作完整 event，削弱文档承诺的截断拒绝。

## 证据

`echo-integration/src/providers/client.rs` 的 decoder finish 与 stream EOF 逻辑构成源码反例。

## 处理记录

commit `e842d87bb787fb0b1fd39afbc04f834df07aec84`让共享decoder对所有
delimiterless残余失败关闭；三provider入口与合法framing/取消反例通过独立复审。
