---
schema_version: 1
id: finding.streaming-tool-validation
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: trigger_input
focus: [result_side_effect, contract_evidence, failure_concurrency]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions]
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Streaming Tool 路径跳过统一参数校验

## 问题

ToolManager 非流式路径执行 schema 与 `Tool::validate_parameters`，流式路径取得 Tool 后直接执行，形成两套输入合同。

## 触发条件与影响

相同无效参数经 streaming 调用可能到达外部 effect，而非流式调用会在执行前拒绝。

## 证据

`echo-execution/src/tools.rs` 的 execute 与 execute_stream 路径提供生产反例。

## 处理记录

Discovery 记录；后续 audit 应复用单一 validation kernel 并补 stream/non-stream 对等测试。
