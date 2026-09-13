---
schema_version: 1
id: finding.streaming-tool-validation
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: trigger_input
focus: [result_side_effect, contract_evidence, failure_concurrency]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.streaming-tool-validation-repair, evidence.streaming-tool-validation-verification]
audit_refs: [audit.tool-permission-sandbox.failure-concurrency, audit.streaming-tool-validation-rereview]
decision_refs: []
repair_evidence_refs: [evidence.streaming-tool-validation-repair]
verification_evidence_refs: [evidence.streaming-tool-validation-verification]
rereview_audit_refs: [audit.streaming-tool-validation-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Streaming Tool 路径跳过统一参数校验

## 问题

基准ToolManager非流式路径执行schema与`Tool::validate_parameters`，流式路径取得Tool后直接执行；真实stream tests确认非法输入到达Tool。

## 触发条件与影响

相同无效参数经 streaming 调用可能到达外部 effect，而非流式调用会在执行前拒绝。

## 证据

当前三个入口复用唯一schema后custom kernel，stream在cache、permit与Tool future前返回同类typed error；调用计数证明非法输入未执行。

## 处理记录

确定性red/green、25个ToolManager回归、Clippy/check与独立复审已闭合本Finding；custom validator自身行为继续由Tool合同约束。
