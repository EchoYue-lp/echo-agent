---
schema_version: 1
id: finding.structured-output-schema-validation-contract
kind: finding
type: intent_gap
status: open
severity: high
primary_focus: contract_evidence
focus: [trigger_input, state_authority]
boundary_ref: boundary.llm-provider-runtime
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.llm-provider-runtime.contract-evidence]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# strict structured output 没有 framework schema validation

## 问题

JsonSchemaSpec strict 与文档承诺严格 schema，但 extract_json 只检查 JSON 语法，extract/execute_typed 只做 serde 反序列化，没有本地 JSON Schema validation。

## 触发条件与影响

Provider 忽略 schema 或 custom LlmClient 错报能力时，结构不符合所声明 schema 的合法 JSON 可被当作成功。

## 证据

Structured output types、`src/agent/react/extract.rs` 与正式文档展示 strict 字段和实际验证边界。

## 处理记录

Contract Audit 确认 intent gap；需 semantic-decide 裁决 strict 是 provider hint 还是 framework 端到端保证。
