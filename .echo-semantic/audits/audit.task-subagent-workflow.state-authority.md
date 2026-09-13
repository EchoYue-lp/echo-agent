---
schema_version: 1
id: audit.task-subagent-workflow.state-authority
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: state_authority
freshness: examined
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.task-patch-claim-race, finding.task-subagent-attempt-link, finding.subagent-factory-cancellation, finding.subagent-factory-publication-race, finding.subagent-definition-catalog]
challenges:
  relation-patch-versus-live-claim:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-orchestration/src/tasks/revisioned.rs, echo-orchestration/src/tasks/runtime_service.rs]
    evidence_refs: [evidence.task-subagent-workflow]
  task-claim-to-subagent-attempt:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/agent/subagent/team/mod.rs, echo-sdk-host/src/core_profile/facade/task_runtime.rs, src/agent/subagent/executor.rs, echo-core/src/agent/event_envelope.rs]
    evidence_refs: [evidence.task-subagent-workflow]
  subagent-factory-publication:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/agent/subagent/registry.rs, echo-orchestration/src/tasks/runtime_executor.rs]
    evidence_refs: [evidence.task-subagent-workflow]
---

# Task 与 Subagent 状态权威审计

## 审查范围

审查 relation revision 与 live claim、TaskClaim 到 SubagentAttempt identity、lazy factory publication/cancellation 和 definition/executable catalog。

## 已检查故障假设

验证 patch 是否覆盖并发 claim、Team/SDK adapter 是否丢弃 claim identity、factory future drop/发布窗口是否遗留或双创建实例，以及 definition-only 是否被错误广告为可执行。

## 实际实现路径与证据

Patch 可在旧 snapshot 上提交，`SetStatus`/`Skip` 又进入 execution drift 豁免集合，确认 live claim 覆盖竞态。Team 与 SDK controller 都丢弃 claim 并调用普通 dispatch，虽然 Executor 已提供 typed `dispatch_attempt`。Factory await 前写入 `instantiating`，取消/abort 不清理；正常完成时又先删除标记再取得 state lock，存在第二次 factory 启动窗口。Catalog 文档宣称 definition-only 可见，代码和测试却过滤它。

## 问题记录

四个既有 Finding 均确认并保持 open；新增 `finding.subagent-factory-publication-race`。Definition catalog 是 intent gap，需人裁决“executable-only”还是另建 discovery catalog。

## 残余风险

Factory cancellation 与 publication race 可共用 RAII publication guard 修复，但需分别保留确定性交错测试；Task/Attempt link 需覆盖 lineage/event/control。

## 未检查项

未执行自定义并发复现；未审查外部持久 RevisionedTaskStore、应用层 Task adapter 或跨进程 factory recovery。
