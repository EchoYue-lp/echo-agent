---
schema_version: 1
id: finding.improve-single-case-panic
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: trigger_input
focus: [failure_concurrency, contract_evidence]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: [audit.eval-evolution.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Improve 单 case criteria 分组可触发 panic

## 问题

ImprovementLoop 对每个 criteria 分组执行 `split_idx.clamp(1, group.len().saturating_sub(1))`；单 case 分组产生下界 1、上界 0，触发 panic。

## 触发条件与影响

合法 eval 输入中某个 criteria 只有一个 case 时，离线 improvement 进程会 panic，而不是返回 typed error 或可用的 train/holdout disposition。

## 证据

`src/improve/loop.rs` 的分组与 split 逻辑构成可复现源码反例。

## 处理记录

Discovery 记录；后续 repair 需先固定单例分组预期并增加无 panic 边界测试。
