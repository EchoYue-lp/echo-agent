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
audit_refs: [audit.llm-provider-runtime.failure-concurrency, audit.llm-provider-runtime.time-lifecycle]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Non-stream LLM cancellation 不对等

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/69

## 问题

ChatRequest cancel_token 声称中止 in-flight request；OpenAI Chat 与 Responses non-stream 完全未监听，Anthropic 只在响应头前监听、body JSON 读取不可取消。

## 触发条件与影响

取消非流式调用时，不同 provider 可能立即停止或继续网络请求并消耗资源，破坏统一生命周期合同。

## 证据

`echo-core/src/llm/mod.rs` 和三种 provider non-stream 实现展示不同行为。

## 处理记录

Discovery 记录；下一阶段用同一 cancellation fixture 覆盖所有 provider adapter。
