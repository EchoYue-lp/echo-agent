---
schema_version: 1
id: audit.task-subagent-attempt-link-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: state_authority
freshness: examined
revision: f310825c418932cde02f661f0686f83f216771d6
finding_refs: [finding.task-subagent-attempt-link]
challenges:
  exact-attempt-authority:
    revision: f310825c418932cde02f661f0686f83f216771d6
    source_refs: [echo-orchestration/src/tasks/runtime.rs, echo-orchestration/src/tasks/runtime_executor.rs, src/agent/subagent/team/mod.rs]
    evidence_refs: [evidence.task-subagent-attempt-link-repair, evidence.task-subagent-attempt-link-verification]
  cancellation-and-recovery:
    revision: f310825c418932cde02f661f0686f83f216771d6
    source_refs: [echo-orchestration/src/tasks/runtime_service.rs, src/agent/subagent/control.rs, src/agent/subagent/executor.rs]
    evidence_refs: [evidence.task-subagent-attempt-link-repair, evidence.task-subagent-attempt-link-verification]
---

# TaskClaim 与 SubagentAttempt framework 独立复审

## 审查范围

独立 reviewer 读取完整 #99 framework diff、Design、Plan、ADR 0058，以及 Task runtime、
Subagent control/executor、Team runtime、tests 与 focused 验证结果。SDK worktree 与其它 OpenCode
并行 finding 被排除。

## 已检查故障假设

检查了 identity 被 adapter 重建、同 run 的 Team handle 覆盖、exact cancel 波及 sibling、
pre-admission intent 丢失、JoinSet completion 归因错位、grace 在 handle 出现前耗尽、join 后 CAS
前伪报 active、settlement authority unknown 导致 waiter 永久挂起、恢复被 live hook 阻塞、joined
projection 泄漏，以及 cleanup failure 反转 durable terminal。

## 实际实现路径与证据

复审沿 `RuntimeTaskService -> RuntimeDagExecutor -> TeamDispatchController ->
SubagentExecutor/SubagentControlRegistry` 的真实路径检查 durable claim、live projection、
JoinSet supervision 与 recovery，而不是只检查类型或单元 helper。repair 与 verification
evidence 分别绑定实现 checkpoint 与定向命令。

## 问题记录

前两轮复审发现 join-to-CAS 假 `ActiveRequested`、authority-unknown waiter 无法释放，以及恢复
顺序/本地 projection 退休缺口。逐项修复后，第三次 recovery 增量复审绑定 diff
`737d17a05839cb7052eca1b5fc5657e8efd6cc3dbc7f3e0a576f05c7c1d7b30d`，结论为 pass，
Critical 0、Important 0、Minor 0。

## 残余风险

已被远端接纳的外部 effect 不能由本地 cancellation 撤回。完整合并门禁、feature matrix、
semantic strict gate 和远端 CI 仍需执行；SDK Host command replay/E2E 未交付，因此 Finding 与
GitHub Issue #99 继续保持 open。

## 未检查项

未执行 SDK Host durable command replay、跨仓库 E2E、真实远端 provider/effect 撤回或 EKO
GUI/TUI 投影。完整 workspace、feature matrix 与远端 CI 留给最终交付门禁。
