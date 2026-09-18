---
schema_version: 1
id: audit.task-subagent-external-control-handle-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: state_authority
freshness: examined
revision: 0415ba15eb8d348f357fe55df4448897677e6960
finding_refs: [finding.task-subagent-attempt-link]
challenges:
  scope-bound-live-authority:
    revision: 0415ba15eb8d348f357fe55df4448897677e6960
    source_refs: [src/agent/subagent/control.rs, src/agent/subagent/executor.rs, src/agent/subagent/team/mod.rs]
    evidence_refs: [evidence.task-subagent-external-control-handle-repair, evidence.task-subagent-external-control-handle-verification]
  external-context-preservation:
    revision: 0415ba15eb8d348f357fe55df4448897677e6960
    source_refs: [src/agent/subagent/executor.rs, tests/facade_smoke.rs]
    evidence_refs: [evidence.task-subagent-external-control-handle-verification]
---

# External Task adapter attempt-control handle 独立复审

## 审查范围

独立reviewer读取plan_02、相关Design章节、完整未提交diff、control registry、executor、Team两条真实
主路径、public facade与定向验证日志；SDK与其它OpenCode worktree被排除。

## 已检查故障假设

检查raw registry泄漏、durable claim authority被handle替代、跨scope同execution消费reservation或取消/
retire、同scope跨run假冲突与错误reconcile、错误scope settlement遗留索引、context identity覆盖，以及
conversation/isolation/message/trace/resource guards/uplink丢失。

## 实际实现路径与证据

初审发现registry命中execution后只校验attempt，导致另一scope handle可跨scope控制；同时task-attempt key
缺run维度。修复后reserve/admit/interrupt/retire/attach在mutation前校验完整identity，detach/settle只接受
完全相同identity，task-attempt key与reconcile均按scope+run隔离。新增负向测试与完整字段保留测试通过。

## 问题记录

第一轮一个Important finding已直接修复；增量复审结论pass，阻断项0。one-shot
`execute_team(TeamDispatchFn)`按Design保留为不承诺并发control的legacy convenience，不属于隐藏权威主路径。

## 残余风险

public facade没有独立构造真实`TaskSubagentContext`完成全生命周期，但registry、binder、programmatic Team与
React Team真实路径已直接覆盖。SDK pin、durable Task store、command journal与crash replay仍未交付。

## 未检查项

完整workspace合并门禁与17-feature matrix已由主流程通过。未检查远端CI或SDK Host跨进程E2E；前者由
PR交付核实，后者属于delivery map的后续Outcome。
