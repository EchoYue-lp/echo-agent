---
schema_version: 1
id: finding.provider-capability-authority
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: state_authority
focus: [contract_evidence, failure_concurrency, trigger_input]
boundary_ref: boundary.llm-provider-runtime
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.model-facts-authority-repair, evidence.model-facts-authority-verification]
audit_refs: [audit.llm-provider-runtime.contract-evidence, audit.llm-provider-runtime.time-lifecycle]
decision_refs: []
repair_evidence_refs: [evidence.model-facts-authority-repair]
verification_evidence_refs: [evidence.model-facts-authority-verification]
rereview_audit_refs: [audit.model-facts-freshness-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Provider capabilities 与 ModelProfile authority 未闭合

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/77

## 问题

LlmClient capabilities 默认 OpenAI-compatible，concrete providers 未 override；LlmConfig 构造 client 也不自动建立 ModelProfile，多个策略源可产生冲突。

## 触发条件与影响

Anthropic 等 provider 被当作支持 JSON response format 或其它能力时，compression/Agent 可发送不被接受的请求。

## 证据

`echo-core/src/llm/mod.rs`、`llm/capabilities.rs`、provider implementations、`echo-state` SummaryCompressor 和 builder 路径提供证据。

## 处理记录

ADR 0045 DU-68与ADR 0047固定唯一合成顺序；LlmClient默认保守，具体adapter固定protocol baseline，动态provider/exact/caller facts通过同一resolver并由现有capabilities consumer读取fresh profile。独立 rereview Audit 已覆盖 provider/protocol cross-product、历史 label compatibility、unknown provider isolation 与 fresh-wins merge；structured output 和 tokenizer dispatch 等后续边界保持独立。
