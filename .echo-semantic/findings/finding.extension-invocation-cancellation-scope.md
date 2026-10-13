---
schema_version: 1
id: finding.extension-invocation-cancellation-scope
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: state_authority
focus:
- failure_concurrency
- data_durability
- time_lifecycle
boundary_ref: boundary.protocol-surfaces
behavior_refs:
- behavior.protocol-projection
rule_refs:
- rule.protocol-role-separation
evidence_refs:
- evidence.task-runtime-store-control-repair
- evidence.task-runtime-store-control-verification
- evidence.framework-parent-drop-final-verification
- evidence.framework-final-34d5-full-verification
audit_refs:
- audit.extension-invocation-cancellation-scope-rereview
- audit.framework-parent-drop-rereview
- audit.framework-final-34d5-gate-rereview
decision_refs: []
repair_evidence_refs:
- evidence.task-runtime-store-control-repair
verification_evidence_refs:
- evidence.task-runtime-store-control-verification
- evidence.framework-parent-drop-final-verification
- evidence.framework-final-34d5-full-verification
rereview_audit_refs:
- audit.extension-invocation-cancellation-scope-rereview
- audit.framework-parent-drop-rereview
- audit.framework-final-34d5-gate-rereview
discovered_at: 95ec66c6083a8feb17a1c28de0d01e1a24ca8a28
---


# 扩展 invocation 取消误传播到父与 sibling

## 问题

lease 直接保留 passed token，SDK stream drop 取消它会取消整个 connection/run 与独立 sibling。

## 触发条件与影响

同 parent token 创建 A/B lease，取消/timeout/drop A 或 cancel_all，旧实现令 parent/B 同时取消。

## 证据

见修复 Evidence 中逐反例日志 SHA、可执行回归，以及最终完整门禁 Evidence。

## 处理记录

本地 child cancellation 修复与完整门禁已完成。2026-10-12 独立审者重新复审当前完整候选并返回 pass，核对三个旧实现反例、13 个 ACP extension 测试和全部门禁日志。本地 repair、verification、rereview 已归档，Finding resolved；SDK transport 和三语言生命周期仍单独验收，未创建/关闭外部 Issue。


本地 Finding 状态与远端 main/Issue 交付独立；后者由协调线程另行对账，不作为本地实现问题已修复与否的替代权威。

本地问题当前闭合：新组合 23 门禁、精确源码及独立复审 pass/action0 已归档；历史证据保持各自快照。公开 pin、SDK/CLI/官网与远端 main 为独立交付义务，本地 resolved 不表示它们完成。


## 当前候选最终复审

audit.framework-final-34d5-gate-rereview 将本地此故障的精确源码/既有红绿证据与同source34d5新完整23门禁绑定；独立implementation review PASS/action0。当前可以进入授权framework里程碑交付，SDK/remoteCI/公开pin及精确main仍另验，其它Finding与全risk lenses不随此关闭。
