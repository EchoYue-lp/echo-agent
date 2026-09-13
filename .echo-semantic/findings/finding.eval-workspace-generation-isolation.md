---
schema_version: 1
id: finding.eval-workspace-generation-isolation
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [result_side_effect, time_lifecycle, state_authority]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution]
rule_refs: [rule.quality-observation-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.eval-workspace-generation-repair, evidence.eval-workspace-generation-verification]
audit_refs: [audit.eval-evolution.failure-concurrency, audit.eval-workspace-generation-rereview]
decision_refs: []
repair_evidence_refs: [evidence.eval-workspace-generation-repair]
verification_evidence_refs: [evidence.eval-workspace-generation-verification]
rereview_audit_refs: [audit.eval-workspace-generation-rereview]
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

Issue #50追踪。唯一generation guard已覆盖同ID/无fixture并发、settled close、cleanup error、timeout/caller-drop retain及Improve/Comparator收敛；red/green、ADR、双语文档与两轮独立复审闭合本Finding。GitHub Issue保持open，等待远端main交付。
