---
schema_version: 1
id: finding.structured-output-main-path
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: contract_evidence
focus: [trigger_input, result_side_effect]
boundary_ref: boundary.llm-provider-runtime
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.structured-output-main-path-repair, evidence.structured-output-main-path-verification]
audit_refs: [audit.llm-provider-runtime.contract-evidence]
decision_refs: []
repair_evidence_refs: [evidence.structured-output-main-path-repair]
verification_evidence_refs: [evidence.structured-output-main-path-verification]
rereview_audit_refs: [audit.structured-output-terminal-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Agent structured output 配置未进入主 ReAct request

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/96

## 问题

Builder/AgentConfig 保存 response_format，主 think request 固定为 None；execute_typed 只在普通 execute 后反序列化，不会让 provider 按 schema 生成。

## 触发条件与影响

调用方配置 structured output 或 execute_typed 时，模型请求不携带约束，导致文档化的自动格式保证不可达。

## 证据

`src/agent/react/builder.rs`、`src/agent/config.rs`、`src/agent/react/run/phases/think.rs` 与 `src/agent/react/extract.rs` 构成直接反例。

## 处理记录

ReAct run snapshot 现在同时持有 caller 声明的 `response_format` 和新鲜 model
capability；每次主模型请求携带 JSON 格式，显式 Text 映射为无约束请求。未知或不支持
结构化输出的模型在请求前失败，最终输出由框架本地校验。ADR 0078/0079、repair、
verification 和独立复审均已记录；外部 Issue 须等远端 main 交付后关闭。
