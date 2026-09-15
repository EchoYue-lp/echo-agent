---
schema_version: 1
id: evidence.workflow-checkpoint-claim-settlement-repair
kind: evidence
observed_at: eb8744566dcd5a734531869ebde9f3506b132163
source_refs:
  - echo-orchestration/src/workflow/checkpoint_store.rs
  - echo-orchestration/src/workflow/graph.rs
  - echo-sdk-protocol/src/methods.rs
  - echo-sdk-host/src/core_profile/extension_bridge.rs
  - docs/adr/0040-workflow-checkpoint-lease-and-sibling-settlement.md
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - Renewable lease防止活动owner被年龄回收，但不承诺外部effect与ack之间崩溃时exactly-once
  - Workflow四执行入口的事件归约仍由finding.workflow-entry-loop-drift追踪
---

# Workflow checkpoint claim结算修复证据

## 支持的结论

`CheckpointStore`继续作为唯一claim authority。默认renew/ack/requeue不再成功no-op，Graph在
claim前拒绝不完整settlement store；claim必须返回attempt identity，validation、执行失败和
heartbeat失败都使用同一attempt requeue，成功才ack。

FileStore把checkpoint与renewed_at保存在私有claim wrapper中，在跨实例file lock内判断
stale。Graph按Store声明的interval续租；requeue/stale recovery先在claim路径原子写入
owner-cleared checkpoint，再rename发布pending，旧attempt不能ack、renew或requeue。Tag只通过
pending generation CAS，不能复活活动claim。

SDK AgentComponent descriptor协商heartbeat interval，Rust Host及TypeScript、Python、Java
均无损传输save-if-generation、claim、renew、requeue与ack，不再由Host猜测远端lease。

## 来源与范围

实现固定于commit `eb8744566dcd5a734531869ebde9f3506b132163`，复用现有
CheckpointStore、Graph与AgentComponent bridge，没有引入第二checkpoint store或resume loop。

## 已知缺口

崩溃发生在不可回滚外部effect之后、ack之前时仍可能重放；本合同是renewable at-least-once
claim，不是全局exactly-once effect transaction。
