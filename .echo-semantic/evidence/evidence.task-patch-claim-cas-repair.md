---
schema_version: 1
id: evidence.task-patch-claim-cas-repair
kind: evidence
observed_at: source:64131952ceb6f498fe94fc34482afe3ecf1e1e77f5a1d31a6ff3ce81b7e0eb01
source_refs:
  - echo-orchestration/src/tasks/revisioned.rs
  - docs/adr/0008-canonical-runtime-task-authority.md
supports: [behavior.task-subagent-execution, rule.task-subagent-authority]
limitations:
  - 只修复 relation patch 与同进程 runtime mutation 的条件提交竞态，不证明外部 RevisionedTaskStore 实现已遵守新增 precondition
  - 不关闭 TaskClaim 到 SubagentAttempt 关联、Subagent factory 或 Workflow 的相邻 Finding
---

# Task relation patch 与 live claim 的 CAS 修复证据

## 支持的结论

基准 `6d55fae97367dedb690d9a7d865ed97d0038b253` 仅以 graph revision 和 patch effects 判断 execution drift；确定性交错证明旧 relation patch 可在 claim 后继续提交并清除 live claim。当前实现让 canonical `TaskRevisionService` 在读取 graph 时捕获完整 `TaskId -> TaskExecution`，并由 `RevisionedTaskStore::compare_and_commit` 在 graph revision 校验后执行 exact compare。claim、retry、settlement、status、error 或 task 集合变化都会返回既有 typed `Conflict`，不会写入 stale snapshot。

## 来源与范围

`echo-orchestration/src/tasks/revisioned.rs` 是 relation patch、runtime claim 与内存 Store 的唯一 Task authority；ADR 0008 将 execution snapshot 明确为 conditional commit precondition。`TaskGraphCommit.expected_executions` 是条件输入，不是第二份持久状态；create 与 legacy direct commit 的 `None` 兼容分支不作为安全 patch 路径。

## 已知缺口

本切片可通过回退其单一任务提交恢复基准行为。外部 Store 复用方必须在实现 `compare_and_commit` 时尊重该字段；其跨进程原子性仍需各 Store 自身的集成证据。
