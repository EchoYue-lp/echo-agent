---
schema_version: 1
id: finding.improve-single-case-panic
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: trigger_input
focus: [failure_concurrency, contract_evidence]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.improve-singleton-split-repair, evidence.improve-singleton-split-verification]
audit_refs: [audit.eval-evolution.failure-concurrency, audit.improve-singleton-split-rereview]
decision_refs: []
repair_evidence_refs: [evidence.improve-singleton-split-repair]
verification_evidence_refs: [evidence.improve-singleton-split-verification]
rereview_audit_refs: [audit.improve-singleton-split-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Improve 单 case criteria 分组可触发 panic

## 问题

基准ImprovementLoop对每个criteria分组执行`split_idx.clamp(1, group.len().saturating_sub(1))`；真实单case测试确认下界1、上界0触发panic。

## 触发条件与影响

合法 eval 输入中某个 criteria 只有一个 case 时，离线 improvement 进程会 panic，而不是返回 typed error 或可用的 train/holdout disposition。

## 证据

当前单例采用train-only disposition，多例group保持train/holdout各至少一项；迭代分配消除动态slice并保证每个case恰好出现一次。

## 处理记录

确定性red/green、ratio与混合group边界、独立review及语义Evidence已闭合本Finding；全singleton无独立泛化分数作为残余限制保留。
