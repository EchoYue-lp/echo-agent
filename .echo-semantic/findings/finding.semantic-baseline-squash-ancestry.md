---
schema_version: 1
id: finding.semantic-baseline-squash-ancestry
kind: finding
type: evidence_gap
status: resolved
severity: high
primary_focus: contract_evidence
focus: [data_durability]
boundary_ref: boundary.workspace-architecture
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure, evidence.semantic-baseline-squash-ancestry-repair, evidence.semantic-baseline-squash-ancestry-verification]
audit_refs: [audit.workspace-architecture.contract-evidence, audit.semantic-baseline-squash-ancestry-rereview]
decision_refs: []
repair_evidence_refs: [evidence.semantic-baseline-squash-ancestry-repair]
verification_evidence_refs: [evidence.semantic-baseline-squash-ancestry-verification]
rereview_audit_refs: [audit.semantic-baseline-squash-ancestry-rereview]
discovered_at: d492c676d1bf0744452d96a6960124546ed3fff9
---

# Squash merge后semantic baseline祖先引用失效

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/118

## 问题

PR #117 squash merge后，repository baseline仍把任务分支中间提交`7ba1f122`作为`base_revision`。该Git对象仍可恢复，但不再是远端main `d492c676`的祖先，strict snapshot因此fail closed。

## 触发条件与影响

任何在squash合并后的main运行strict、change-evidence或continuity的任务都会先被无效ancestor阻断，导致治理材料只在原线性任务分支成立。

## 证据

`git merge-base --is-ancestor 7ba1f122 d492c676`返回1，而`b21aba01`和`d492c676`对当前main均返回0。Post-merge strict稳定报告`基准 revision 不是当前 HEAD 的祖先`。

## 处理记录

Issue #118已建立。Baseline YAML和正文共同绑定已合并main`d492c676`，learning contract与Rust CI在合并前要求baseline revision属于target main；source digest只因测试/CI/AGENTS刷新，closure、runtime/API和SDK不变。完整门禁与独立复审均通过，本Finding在本地语义层resolved；Issue保持OPEN直到follow-up进入远端main。
