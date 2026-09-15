---
schema_version: 1
id: audit.skill-activation-authority-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: state_authority
freshness: examined
revision: 8b912e97840b000adf6d844c805508aa5f18a2c8
finding_refs: [finding.skill-activation-authority]
challenges:
  checkpoint-and-policy-authority:
    revision: 8b912e97840b000adf6d844c805508aa5f18a2c8
    source_refs: [src/agent/snapshot.rs, src/agent/react/mod.rs, echo-execution/src/skills/registry.rs]
    evidence_refs: [evidence.skill-activation-authority-repair, evidence.skill-activation-authority-verification]
  definition-reconciliation:
    revision: 8b912e97840b000adf6d844c805508aa5f18a2c8
    source_refs: [src/agent/react/capabilities.rs, echo-sdk-host/src/core_profile/facade/source_operations.rs, src/agent/react/tests.rs]
    evidence_refs: [evidence.skill-activation-authority-repair, evidence.skill-activation-authority-verification]
  activation-effect-singleflight:
    revision: 8b912e97840b000adf6d844c805508aa5f18a2c8
    source_refs: [echo-execution/src/skills/registry.rs, echo-execution/src/skills/external/activate_tool.rs]
    evidence_refs: [evidence.skill-activation-authority-repair, evidence.skill-activation-authority-verification]
  public-contract-classification:
    revision: 8b912e97840b000adf6d844c805508aa5f18a2c8
    source_refs: [echo-sdk-protocol/src/facade.rs, docs/adr/0044-skill-activation-authority.md]
    evidence_refs: [evidence.skill-activation-authority-repair, evidence.skill-activation-authority-verification]
---

# Skill activation authority 独立复审

## 审查范围

复审canonical activation handle、checkpoint/telemetry读取、descriptor-derived policy恢复、
definition reconciliation、SDK mutation、single-flight、取消与reset/remove fencing、
replacement rollback和Rust/Host-only contract分类。

## 已检查故障假设

验证旧telemetry Vec是否复活状态、restore是否丢policy、SDK mutation是否分叉definition、
双activation是否重放effect、取消/reset/remove是否允许旧future发布、deny replacement是否先
破坏旧代，以及cross-crate construction seam是否逃逸inventory。

## 实际实现路径与证据

单一handle原子保存epoch、generation、active content/policy与flight；snapshot直接持有handle。
SDK definition mutation进入prevalidate-then-swap Agent API；same-key共享flight，cancel/panic
poison，reset/remove/replacement fence旧代。Public seam有显式process-local classifier。

Reviewer首轮阻断四类权威缺口，第二轮阻断policy deny rollback；修复并通过focused tests后，
最终复审未发现Critical、Important或Minor问题，结论PASS。

## 问题记录

`finding.skill-activation-authority`具备repair、verification与独立rereview证据，可标记resolved。
GitHub Issue #93等待修复进入远端main后关闭。

## 残余风险

恢复的active Skill没有进程内cached content，显式reactivation会fail closed直到reset；最终组合
SDK inventory与完整门禁留给集成分支统一执行。

## 未检查项

未执行完整workspace gate、全部feature矩阵、最终SDK artifact、远端CI或其它open Finding；
Host all-feature e2e曾因并行worktree磁盘耗尽在编译期终止，未形成行为失败结论。
