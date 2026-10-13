---
schema_version: 1
id: finding.task-runtime-store-control
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: state_authority
focus:
- failure_concurrency
- data_durability
- time_lifecycle
boundary_ref: boundary.task-subagent-workflow
behavior_refs:
- behavior.task-subagent-execution
rule_refs:
- rule.task-subagent-authority
evidence_refs:
- evidence.task-runtime-store-control-repair
- evidence.task-runtime-store-control-verification
- evidence.framework-parent-drop-final-verification
- evidence.framework-final-34d5-full-verification
audit_refs:
- audit.task-runtime-store-control-rereview
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
- audit.task-runtime-store-control-rereview
- audit.framework-parent-drop-rereview
- audit.framework-final-34d5-gate-rereview
discovered_at: 95ec66c6083a8feb17a1c28de0d01e1a24ca8a28
---


# 注册 Store 的 Task 执行、结果与精确中断权威缺口

## 问题

基准任意 registered Store 仅参与关系 CRUD，执行仅有 concrete InMemory 入口；结果与 Completed 分离，局部停止升级 root，中断请求/结果/关系 patch 竞态，joined attempt 与 watcher 的生命周期未闭合。

## 触发条件与影响

自定义 Store 运行 Task、取消同 wave 的一个节点、暂停/恢复、慢 review/CAS、父 future 撤销、post-CAS cleanup 与公开 reconcile 交错，可能导致双图、结果丢失、无关任务停止、错误重跑、未知 watcher 永等待或真实 receipt 降级。

## 证据

见修复 Evidence 中逐反例日志 SHA、可执行回归，以及最终完整门禁 Evidence。

## 处理记录

本地修复与完整门禁已完成。2026-10-12 独立审者对基准到当前完整候选重新复审并返回 pass，核验门禁后 runtime/test 未变、ADR 仅状态格式变化及语义材料引用。repair、verification 与 rereview 已归档，本地 Finding resolved；未创建/关闭外部 Issue，不声明 SDK/CLI consumer 或远端 main 已完成。


本地 Finding 状态与远端 main/Issue 交付独立；后者由协调线程另行对账，不作为本地实现问题已修复与否的替代权威。

本地问题当前闭合：新组合 23 门禁、精确源码及独立复审 pass/action0 已归档；历史证据保持各自快照。公开 pin、SDK/CLI/官网与远端 main 为独立交付义务，本地 resolved 不表示它们完成。


## 当前候选最终复审

audit.framework-final-34d5-gate-rereview 将本地此故障的精确源码/既有红绿证据与同source34d5新完整23门禁绑定；独立implementation review PASS/action0。当前可以进入授权framework里程碑交付，SDK/remoteCI/公开pin及精确main仍另验，其它Finding与全risk lenses不随此关闭。
