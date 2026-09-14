---
schema_version: 1
id: finding.subagent-factory-cancellation
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, state_authority, contract_evidence]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow, evidence.subagent-factory-singleflight-repair, evidence.subagent-factory-singleflight-verification]
audit_refs: [audit.task-subagent-workflow.state-authority, audit.subagent-factory-singleflight-rereview]
decision_refs: []
repair_evidence_refs: [evidence.subagent-factory-singleflight-repair]
verification_evidence_refs: [evidence.subagent-factory-singleflight-verification]
rereview_audit_refs: [audit.subagent-factory-singleflight-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Subagent lazy factory 取消后无法恢复 publication

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/29

## 问题

基准实现的`SubagentRegistry::get_agent`在等待factory create前把名称加入`instantiating`，清理只发生在await正常返回后；确定性red确认future被abort时名称永久残留。

## 触发条件与影响

Factory create 期间发生 caller cancel、runtime grace timeout 或 task abort 后，后续相同名称 resolve 只能反复等待并超时，Subagent capability 无法自行恢复。

## 证据

当前entry以Tokio OnceCell持有初始化ownership；取消、错误或panic不初始化cell，后续resolve可重试。Registry与executor定向测试及ADR 0033提供验证。

## 处理记录

revision/cell双重fence、确定性red/green与独立复审已闭合本Finding；factory自身的外部副作用清理仍由实现方负责。
