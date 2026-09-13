---
schema_version: 1
id: finding.nonstream-cancellation-parity
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: time_lifecycle
focus: [failure_concurrency, contract_evidence]
boundary_ref: boundary.llm-provider-runtime
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Non-stream LLM cancellation 不对等

## 问题

ChatRequest cancel_token 声称中止 in-flight request；Anthropic non-stream 监听，OpenAI Chat 与 Responses non-stream 未监听。

## 触发条件与影响

取消非流式调用时，不同 provider 可能立即停止或继续网络请求并消耗资源，破坏统一生命周期合同。

## 证据

`echo-core/src/llm/mod.rs` 和三种 provider non-stream 实现展示不同行为。

## 处理记录

Discovery 记录；下一阶段用同一 cancellation fixture 覆盖所有 provider adapter。
