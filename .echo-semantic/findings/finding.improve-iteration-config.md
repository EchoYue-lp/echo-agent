---
schema_version: 1
id: finding.improve-iteration-config
kind: finding
type: implementation_bug
status: open
severity: medium
primary_focus: trigger_input
focus: [contract_evidence, result_side_effect]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.eval-evolution.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# EvalDrivenImprovement 忽略 max_iterations

## 问题

Public max_iterations setter 更新字段，run 却直接构造默认 ImprovementLoop，没有传递该值。

## 触发条件与影响

调用方配置迭代次数时，实际执行仍使用默认值，导致成本、时间和结果不符合请求。

## 证据

`src/improve/eval_improvement.rs` 的字段、setter 与 run 构造路径形成直接反例。

## 处理记录

Discovery 记录；下一阶段补配置传递和边界值测试。
