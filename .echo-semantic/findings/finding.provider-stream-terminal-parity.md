---
schema_version: 1
id: finding.provider-stream-terminal-parity
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, contract_evidence]
boundary_ref: boundary.llm-provider-runtime
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.provider-stream-terminal-parity-repair, evidence.provider-stream-terminal-parity-verification]
audit_refs: [audit.llm-provider-runtime.failure-concurrency, audit.llm-provider-runtime.time-lifecycle, audit.provider-stream-terminal-parity-rereview]
decision_refs: []
repair_evidence_refs: [evidence.provider-stream-terminal-parity-repair]
verification_evidence_refs: [evidence.provider-stream-terminal-parity-verification]
rereview_audit_refs: [audit.provider-stream-terminal-parity-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Provider stream semantic terminal 不对等

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/78

## 问题

Responses 要求 response.completed；OpenAI EOF 不要求 DONE/finish，Anthropic 把 message_stop 当 Other，并在较早 message_delta 暴露 finish_reason/usage。

## 触发条件与影响

OpenAI/Anthropic 流在语义终态前断开时，直接 LlmClient consumer 或 ReAct 可把截断结果、finish reason 与 usage 当作成功。

## 证据

Responses/OpenAI/Anthropic adapters、共享 transport 与 ReAct think terminal 检查展示不对等。

## 处理记录

Failure/Time Audit 确认；后续 repair 为每个 provider 建立 explicit semantic terminal contract 与 EOF tests。

Provider-specific terminal合同、focused fixtures、独立rereview、严格语义验证与完整本地
合并门禁均已闭合；外部Issue只在同一快照进入远端main后关闭。
