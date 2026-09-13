---
schema_version: 1
id: finding.readme-example-target-drift
kind: finding
type: evidence_gap
status: open
severity: medium
primary_focus: contract_evidence
focus: [trigger_input]
boundary_ref: boundary.workspace-architecture
behavior_refs: [behavior.workspace-composition]
rule_refs: [rule.framework-layer-ownership]
evidence_refs: [evidence.workspace-structure]
audit_refs: [audit.workspace-architecture.contract-evidence]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# README把demo34 test contract写成Cargo example target

## 问题

双语README要求`cargo run -p echo-agent-learning --example demo34_workflow_stream`，但该源码是tests/example_contracts模块，Cargo metadata没有同名example target。

## 触发条件与影响

用户复制官方命令会立即失败；现有documentation_contract只检查learning包内部链接/名称，无法发现root README target漂移。

## 证据

README命令、demo34源码、tests/example_contracts.rs与documentation_contract.rs展示真实target和覆盖缺口。

## 处理记录

Workspace Contract Audit确认；属于双语文档/测试合同修复，不改runtime。
