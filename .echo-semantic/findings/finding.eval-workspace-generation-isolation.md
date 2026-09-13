---
schema_version: 1
id: finding.eval-workspace-generation-isolation
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: failure_concurrency
focus: [result_side_effect, time_lifecycle, state_authority]
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

# Eval/Improve workspace缺generation隔离

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/50

## 问题

Eval fixture目录固定为workspace_root/case.id且存在即递归删除；ImprovementLoop固定使用系统tmp/improve_i，early-stop在cleanup前break。

## 触发条件与影响

并发improvement或同名case可互相删除正在使用的workspace，提前成功稳定遗留目录，并污染下一run。

## 证据

`src/eval/runner.rs`与`src/improve/loop.rs`的目录构造、remove与early-stop顺序构成源码反例。

## 处理记录

Failure Audit确认；后续repair使用run/generation唯一目录和scope guard cleanup，并补并发/early-stop测试。
