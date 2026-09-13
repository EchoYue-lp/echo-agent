---
schema_version: 1
id: finding.readme-example-target-drift
kind: finding
type: evidence_gap
status: resolved
severity: medium
primary_focus: contract_evidence
focus: [trigger_input]
boundary_ref: boundary.workspace-architecture
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure, evidence.readme-example-target-repair, evidence.readme-example-target-verification]
audit_refs: [audit.workspace-architecture.contract-evidence, audit.readme-example-target-rereview]
decision_refs: []
repair_evidence_refs: [evidence.readme-example-target-repair]
verification_evidence_refs: [evidence.readme-example-target-verification]
rereview_audit_refs: [audit.readme-example-target-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# README把demo34 test contract写成Cargo example target

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/80

## 问题

双语README要求`cargo run -p echo-agent-learning --example demo34_workflow_stream`，但该源码是tests/example_contracts模块，Cargo metadata没有同名example target。

## 触发条件与影响

用户复制官方命令会立即失败；现有documentation_contract只检查learning包内部链接/名称，无法发现root README target漂移。

## 证据

README命令、demo34源码、tests/example_contracts.rs与documentation_contract.rs展示真实target和覆盖缺口。

## 处理记录

Workspace Contract Audit确认。双语README demo34命令已改为真实`example_contracts` test/filter；Cargo-derived command contract在旧README上red、修复后green，且README原样命令真实通过。Repair、verification与独立复审已闭合本Finding。GitHub Issue保持open，等待远程main交付。
