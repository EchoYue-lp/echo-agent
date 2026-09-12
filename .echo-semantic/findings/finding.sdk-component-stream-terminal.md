---
schema_version: 1
id: finding.sdk-component-stream-terminal
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: time_lifecycle
focus: [contract_evidence, failure_concurrency]
boundary_ref: boundary.sdk-facade-parity
behavior_refs: [behavior.sdk-facade-routing]
rule_refs: [rule.sdk-rust-authority]
evidence_refs: [evidence.sdk-contracts]
audit_refs: [audit.sdk-facade-plan08-final]
decision_refs: []
repair_evidence_refs: [evidence.sdk-contracts]
verification_evidence_refs: [evidence.sdk-contracts]
rereview_audit_refs: [audit.sdk-facade-plan08-final]
discovered_at: source:912701f2c1f489d693198433f91a62cd2e6e530c570a73549bd7b753957ab9de
---

# AgentComponent stream 终态与事件形状混淆

## 问题

第四轮审查发现Sandbox与Workflow组件流曾共用任意WireValue，内层终态可出现在outer chunk，且Workflow extension与graph stream投影不同。

## 触发条件与影响

语言SDK发送组件流时可构造违反Rust终态顺序的payload，消费者也会在同一workflow.stream.next看到两种事件形状。

## 证据

协议现使用Sandbox/Workflow各自的chunk与terminal DTO；协议反例测试拒绝终态/非终态错位，真实Host E2E核对canonical WorkflowEvent投影。

## 处理记录

代码修复与focused验证已完成；第十轮独立复审确认该finding闭合。
