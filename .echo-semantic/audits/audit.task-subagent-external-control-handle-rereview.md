---
schema_version: 1
id: audit.task-subagent-external-control-handle-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: state_authority
freshness: examined
revision: source:0a91548f9c8d3f6e6a19bc2025fd2d21a46b656162f50b44aedfba5b6d5bf1bb
finding_refs: [finding.task-subagent-attempt-link]
challenges:
  scope-bound-live-authority:
    revision: source:0a91548f9c8d3f6e6a19bc2025fd2d21a46b656162f50b44aedfba5b6d5bf1bb
    source_refs: [src/agent/subagent/control.rs, src/agent/subagent/executor.rs, src/agent/subagent/team/mod.rs]
    evidence_refs: [evidence.task-subagent-external-control-handle-repair, evidence.task-subagent-external-control-handle-verification]
  external-context-preservation:
    revision: source:0a91548f9c8d3f6e6a19bc2025fd2d21a46b656162f50b44aedfba5b6d5bf1bb
    source_refs: [src/agent/subagent/executor.rs, tests/facade_smoke.rs]
    evidence_refs: [evidence.task-subagent-external-control-handle-verification]
---

# External Task adapter attempt-control handle 独立复审

## 审查范围

独立reviewer读取plan_02、相关Design章节、完整未提交diff、control registry、executor、Team两条真实
主路径、public facade 与定向验证日志；consumer 与其它并行 worktree 被排除。

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

public facade 没有独立构造真实 `TaskSubagentContext` 完成全生命周期，但 registry、binder、
programmatic Team 与 React Team 真实路径已直接覆盖。Consumer pin、durable Task store、command
journal 与 crash replay 由其所属仓库独立追踪。

## 未检查项

完整 workspace 合并门禁、17-feature matrix、PR #134 与 main-push remote CI 已由主流程通过。
未检查 consumer Host 跨进程 E2E；该项不属于本 framework Finding。

Framework-only closure 在最终 source digest 上确认 public handle 与 canonical registry 仍是一条
live-control 路径，当前文档与 Evidence 没有重新引入 consumer completion blocker。
