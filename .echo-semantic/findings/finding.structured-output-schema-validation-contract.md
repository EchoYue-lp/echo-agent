---
schema_version: 1
id: finding.structured-output-schema-validation-contract
kind: finding
type: intent_gap
status: resolved
severity: high
primary_focus: contract_evidence
focus: [trigger_input, state_authority]
boundary_ref: boundary.llm-provider-runtime
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.structured-output-schema-validation-repair, evidence.structured-output-schema-validation-verification]
audit_refs: [audit.llm-provider-runtime.contract-evidence]
decision_refs: []
repair_evidence_refs: [evidence.structured-output-schema-validation-repair]
verification_evidence_refs: [evidence.structured-output-schema-validation-verification]
rereview_audit_refs: [audit.structured-output-terminal-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# strict structured output 没有 framework schema validation

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/97

## 问题

JsonSchemaSpec strict 与文档承诺严格 schema，但 extract_json 只检查 JSON 语法，extract/execute_typed 只做 serde 反序列化，没有本地 JSON Schema validation。

## 触发条件与影响

Provider 忽略 schema 或 custom LlmClient 错报能力时，结构不符合所声明 schema 的合法 JSON 可被当作成功。

## 证据

Structured output types、`src/agent/react/extract.rs` 与正式文档展示 strict 字段和实际验证边界。

## 处理记录

ADR 0045 DU-97 裁决 strict 为框架端到端保证。一次性提取和主 ReAct 文本、
`final_answer` 工具结果、LlmCritic strict 成功路径均复用本地 validator；错误类型化，
JSON/Schema 错误有界修复，成功终态前校验。ADR 0079 与修复、验证、独立复审证据
已记录；外部 Issue 须等远端 main 交付后关闭。
