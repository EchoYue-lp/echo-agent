---
schema_version: 1
id: finding.subagent-definition-catalog
kind: finding
type: intent_gap
status: resolved
severity: medium
primary_focus: contract_evidence
focus: [trigger_input, state_authority]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: [audit.task-subagent-workflow.state-authority, audit.subagent-definition-catalog-rereview]
decision_refs: []
repair_evidence_refs: [evidence.subagent-definition-catalog-repair]
verification_evidence_refs: [evidence.subagent-definition-catalog-verification]
rereview_audit_refs: [audit.subagent-definition-catalog-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Definition-only Subagent catalog 合同冲突

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/98

## 问题

SubagentRegistry 文档声称 definition-only 项进入 available/catalog，生产实现与测试却过滤没有 instance/factory 的定义。

## 触发条件与影响

Plugin 或配置只注册 definition 时，调用方可能看到与文档不同的可调度列表，影响模型选择和错误分类。

## 证据

`src/agent/subagent/registry.rs` 的注释、过滤实现和相反测试构成直接合同冲突。

## 处理记录

修复采用 hidden-until-resolvable：低层定义仍可通过 `get`/`contains` 检查，模型可见
catalog 和可用列表只展示已绑定实例或 factory 的定义。源码注释、双语文档、测试、
独立复审、严格语义验证与完整本地合并门禁均已闭合；外部Issue只在同一快照进入远端
main后关闭。
