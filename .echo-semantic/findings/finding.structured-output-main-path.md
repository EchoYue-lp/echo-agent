---
schema_version: 1
id: finding.structured-output-main-path
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: contract_evidence
focus: [trigger_input, result_side_effect]
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

# Agent structured output 配置未进入主 ReAct request

## 问题

Builder/AgentConfig 保存 response_format，主 think request 固定为 None；execute_typed 只在普通 execute 后反序列化，不会让 provider 按 schema 生成。

## 触发条件与影响

调用方配置 structured output 或 execute_typed 时，模型请求不携带约束，导致文档化的自动格式保证不可达。

## 证据

`src/agent/react/builder.rs`、`src/agent/config.rs`、`src/agent/react/run/phases/think.rs` 与 `src/agent/react/extract.rs` 构成直接反例。

## 处理记录

Discovery 记录；下一阶段统一 invocation response format 与 extract fallback。
