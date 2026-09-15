---
schema_version: 1
id: audit.scheduler-occurrence-authority-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: failure_concurrency
freshness: examined
revision: 8c8d8aa469233732276df8d313495a5dce67961d
finding_refs: [finding.scheduler-control-fire-race, finding.scheduler-task-id-uniqueness, finding.scheduler-cache-delivery]
challenges:
  control-admission-linearization:
    revision: 8c8d8aa469233732276df8d313495a5dce67961d
    source_refs: [echo-orchestration/src/scheduler/runner.rs]
    evidence_refs: [evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
  unique-definition-identity:
    revision: 8c8d8aa469233732276df8d313495a5dce67961d
    source_refs: [echo-orchestration/src/scheduler/cron_task.rs, echo-orchestration/src/scheduler/runner.rs]
    evidence_refs: [evidence.scheduler-occurrence-authority-repair, evidence.scheduler-occurrence-authority-verification]
  crash-replay-residual:
    revision: 8c8d8aa469233732276df8d313495a5dce67961d
    source_refs: [echo-orchestration/src/scheduler/runner.rs, docs/adr/0042-scheduler-occurrence-authority.md]
    evidence_refs: [evidence.scheduler-occurrence-authority-repair]
---

# Scheduler occurrence authority独立复审

## 审查范围

独立reviewer检查CronTask ID、store/cache刷新、control/admission线性化、definition identity、
remove/re-add fencing和callback settlement；durable occurrence crash replay作为残余单元保留。

## 已检查故障假设

检查重复ID导致控制作用于不同定义、disable/remove成功后旧捕获callback仍被接纳、旧callback
回写新同ID任务、migration覆盖目标Store，以及callback effect与last_run之间崩溃无法判定。

## 实际实现路径与证据

唯一ID、control epoch、definition identity和control-lock admission均有确定性反例。Reviewer确认
#85/#86闭合；因为没有durable claim/ledger，#84明确保持open。

## 问题记录

#85与#86无剩余Critical/Important/Minor；#84不是被忽略的测试缺口，而是独立未实现合同。

## 残余风险

进程崩溃位于callback effect与last_run持久化之间时，系统仍没有明确at-most-once或
at-least-once语义。

## 未检查项

未执行跨进程Store并发或进程崩溃注入。
