---
schema_version: 1
id: finding.provider-capability-authority
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [contract_evidence, failure_concurrency, trigger_input]
boundary_ref: boundary.llm-provider-runtime
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.llm-provider-runtime.contract-evidence, audit.llm-provider-runtime.time-lifecycle]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Provider capabilities 与 ModelProfile authority 未闭合

## 问题

LlmClient capabilities 默认 OpenAI-compatible，concrete providers 未 override；LlmConfig 构造 client 也不自动建立 ModelProfile，多个策略源可产生冲突。

## 触发条件与影响

Anthropic 等 provider 被当作支持 JSON response format 或其它能力时，compression/Agent 可发送不被接受的请求。

## 证据

`echo-core/src/llm/mod.rs`、`llm/capabilities.rs`、provider implementations、`echo-state` SummaryCompressor 和 builder 路径提供证据。

## 处理记录

Discovery 记录；下一阶段确定 provider capability 与 model override 的唯一合成顺序。
