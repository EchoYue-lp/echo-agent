---
schema_version: 1
id: audit.workflow-checkpoint-claim-settlement-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: data_durability
freshness: examined
revision: eb8744566dcd5a734531869ebde9f3506b132163
finding_refs: [finding.workflow-checkpoint-claim-recovery, finding.workflow-checkpoint-resurrection-race]
challenges:
  renewable-attempt-lease:
    revision: eb8744566dcd5a734531869ebde9f3506b132163
    source_refs: [echo-orchestration/src/workflow/checkpoint_store.rs, echo-orchestration/src/workflow/graph.rs]
    evidence_refs: [evidence.workflow-checkpoint-claim-settlement-repair, evidence.workflow-checkpoint-claim-settlement-verification]
  tag-resurrection-fence:
    revision: eb8744566dcd5a734531869ebde9f3506b132163
    source_refs: [echo-orchestration/src/workflow/checkpoint_store.rs, echo-orchestration/src/workflow/graph.rs]
    evidence_refs: [evidence.workflow-checkpoint-claim-settlement-repair, evidence.workflow-checkpoint-claim-settlement-verification]
  remote-settlement-parity:
    revision: eb8744566dcd5a734531869ebde9f3506b132163
    source_refs: [echo-sdk-protocol/src/methods.rs, echo-sdk-host/src/core_profile/extension_bridge.rs]
    evidence_refs: [evidence.workflow-checkpoint-claim-settlement-verification]
---

# Workflow checkpoint claim结算独立复审

## 审查范围

独立reviewer检查Memory/File Store、Graph resume、跨实例file lock、claim wrapper crash cut、
generation CAS、AgentComponent wire/Host proxy及三语言SDK；Graph多入口漂移排除。

## 已检查故障假设

检查claim后失败永久丢失、默认settlement假成功、活动长任务被固定年龄回收、renew与stale
recover跨实例竞态、旧attempt删除新owner、tag复活claim，以及三语言在callback前拒绝新操作。

## 实际实现路径与证据

首次review发现远程Store no-op和固定lease问题；第二次发现file lock/crash publication及SDK
协商缺口；最终实现使用attempt-fenced renewable claim、owner-cleared原子发布和完整四语言
合同，最终review结论pass。

## 问题记录

最终复审Critical 0、Important 0、Minor 0，#109/#110均可关闭。

## 残余风险

外部effect与ack不是同一事务；进程在两者之间崩溃可能按at-least-once语义重放。

## 未检查项

未运行真实远程SDK进程网络分区、kill -9或完整workspace门禁。
