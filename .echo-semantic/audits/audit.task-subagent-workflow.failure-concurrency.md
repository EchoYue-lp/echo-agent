---
schema_version: 1
id: audit.task-subagent-workflow.failure-concurrency
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: failure_concurrency
freshness: examined
revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
finding_refs: [finding.workflow-dag-authority, finding.workflow-entry-loop-drift, finding.workflow-checkpoint-claim-recovery, finding.workflow-checkpoint-resurrection-race, finding.workflow-parallel-failure-settlement]
challenges:
  workflow-entry-parity:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-orchestration/src/workflow/mod.rs, echo-orchestration/src/workflow/graph.rs, echo-orchestration/src/workflow/dag.rs]
    evidence_refs: [evidence.task-subagent-workflow]
  checkpoint-claim-and-resurrection:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-orchestration/src/workflow/checkpoint_store.rs, echo-orchestration/src/workflow/graph.rs]
    evidence_refs: [evidence.task-subagent-workflow]
  parallel-failure-settlement:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-orchestration/src/workflow/graph.rs, echo-orchestration/src/workflow/dag.rs]
    evidence_refs: [evidence.task-subagent-workflow]
---

# Workflow 并发失败与恢复审计

## 审查范围

审查 Graph/DagWorkflow/Task DAG 边界、四个 Graph 执行入口、WorkflowEvent、checkpoint claim/resume/tag 和并行节点失败结算。

## 已检查故障假设

验证多入口在错误/中断/恢复时是否漂移，claim 后失败是否隐藏 continuation，tag 是否复活已领取 checkpoint，以及并行 sibling 是否遮蔽失败或在 caller cancel 后继续 effect。

## 实际实现路径与证据

Graph 的 run/run-until-interrupt/resume/stream 各自维护循环；公开 NodeError 声称非致命继续，但 stream 直接错误退出且没有生产 NodeError。File 与 memory checkpoint 在 claim 返回前已消费，后续 restore/update/execute/save 任一失败都无 ack/requeue。tag_checkpoint 是 load-modify-save，可与 claim 交错复活 checkpoint。Graph join_all 等全部 sibling 后传播错误，无 timeout 时挂起 sibling 可遮蔽失败；DagWorkflow 的 spawned handle 取消/abort 顺序不能可靠停止外部 effect。

## 问题记录

确认三个既有 Finding；新增 checkpoint resurrection 与 parallel failure settlement。Task DAG、Graph、DagWorkflow 的合同不同，consolidation 只能审计共享纯算法或以 ADR 确认 keep-separate，不能建立跨概念统一 runtime。

## 残余风险

NodeError 应继续还是终止、checkpoint 使用 lease+ack 还是 attempt record 仍需设计/裁决；所有 recovery repair 需要 crash-cut 与确定性并发测试。

## 未检查项

未执行故障注入；未验证第三方/多语言 CheckpointStore，也未审查所有 workflow node 的幂等协议。
