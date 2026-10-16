---
{
  "schema_version": 1,
  "id": "audit.skill-activation-authority-rereview",
  "kind": "audit",
  "boundary_ref": "boundary.extension-lifecycle",
  "lens": "state_authority",
  "freshness": "examined",
  "revision": "7110a27e7b0e9841dcfa0d4cca1cdbc053d96177",
  "finding_refs": [
    "finding.skill-activation-authority",
    "finding.framework-historical-source-observation-retention"
  ],
  "challenges": {
    "current-native-source-and-existing-regressions": {
      "revision": "7110a27e7b0e9841dcfa0d4cca1cdbc053d96177",
      "source_refs": [
        "src/agent/snapshot.rs",
        "src/agent/react/mod.rs",
        "echo-execution/src/skills/registry.rs",
        "src/agent/react/capabilities.rs",
        "src/agent/react/tests.rs",
        "echo-execution/src/skills/external/activate_tool.rs",
        "docs/adr/0044-skill-activation-authority.md"
      ],
      "evidence_refs": [
        "evidence.framework-history-current-reobservation"
      ]
    }
  }
}
---

# Current observation and historical record

The machine header now records only the bounded native observation at `7110a27e7b0e9841dcfa0d4cca1cdbc053d96177`; current source/tests and limitations are in `evidence.framework-history-current-reobservation`. Original observation `8b912e97840b000adf6d844c805508aa5f18a2c8` and its complete metadata remain in [the immutable875 document](https://github.com/EchoYue-lp/echo-agent/blob/875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c/.echo-semantic/audits/audit.skill-activation-authority-rereview.md). The original execution counts, before/after claims and review prose below are preserved verbatim as historical statements, not rerun or equivalence claims for7110.

The former F references `echo-sdk-host/src/core_profile/facade/source_operations.rs`, `echo-sdk-protocol/src/facade.rs` belong to the independent SDK accepted-contract owner. These obligations remain in scope there; the common current evidence gives stable019e source links and the old7f14 execution epoch, while new SDK Host/three-language acceptance is unverified here.


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
