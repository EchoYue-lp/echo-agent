---
schema_version: 1
id: finding.improve-iteration-config
kind: finding
type: implementation_bug
status: resolved
severity: medium
primary_focus: trigger_input
focus: [contract_evidence, result_side_effect]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.improve-iteration-config-repair, evidence.improve-iteration-config-verification]
audit_refs: [audit.eval-evolution.failure-concurrency, audit.improve-iteration-config-rereview]
decision_refs: []
repair_evidence_refs: [evidence.improve-iteration-config-repair]
verification_evidence_refs: [evidence.improve-iteration-config-verification]
rereview_audit_refs: [audit.improve-iteration-config-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# EvalDrivenImprovement 忽略 max_iterations

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/31

## 问题

基准public max_iterations setter更新字段，run却直接构造默认ImprovementLoop；真实pipeline测试配置2仍执行5轮。

## 触发条件与影响

调用方配置迭代次数时，实际执行仍使用默认值，导致成本、时间和结果不符合请求。

## 证据

当前run把字段无损传入唯一ImprovementLoop；配置2、0、disabled和empty cases测试覆盖执行与短路边界。

## 处理记录

确定性red/green、Clippy/feature验证与独立复审已闭合本Finding；early-stop可少于上限及真实LLM/report成本保留为限制。
