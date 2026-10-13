---
{
  "schema_version": 1,
  "id": "evidence.task-dispatch-parent-drop-repair",
  "kind": "evidence",
  "observed_at": "68925f49814e50a7074e11cc329c40b00c44d2d1",
  "source_refs": [
    "echo-orchestration/src/tasks/runtime_executor.rs",
    "echo-orchestration/src/tasks/runtime_service.rs",
    "src/agent/subagent/team/mod.rs",
    "docs/adr/0060-task-execution-store-cas-and-local-interruption.md",
    "docs/en/09-tasks.md",
    "docs/zh/09-tasks.md"
  ],
  "supports": [
    "behavior.task-subagent-execution",
    "rule.task-subagent-authority"
  ],
  "limitations": [
    "Focused actual counters prove these cases, not new final23 or SDK consumer/main",
    "Backend commit then unwind without returned exact identity does not authorize stealing/retrying an unknown claim; backend lease/fencing recovery remains its owner responsibility"
  ]
}
---

# 父 execute 退出后的原 driver 与 claim 所有权修复

## 支持的结论

Caller guard 仅取消child，私有driver仍运行同canonical DAG。claim response未完成时不丢CAS；已返回claim立即进入unjoined-wave owner，再awaitreservation。异常driver保真实JoinSet，确认physicalstop后按同Store逐claim结算；单个backend失败不丢其它claim，receipt不可盖。

## 来源与范围

`/tmp/framework-parent-drop-verification.json` 记录真实parked dispatch/heldclaimresponse/reservation旧生产三反例失败与当前通过，以及guarddriverunwind、一Unknown一Settled、未返回identitybackendpanic明确Err/nofakewatch/nodispatch。当前 Task180/Subagent169、正确alltest-Warnings和prod4paniclint、root consumercheck、fmt/diff全部通过；原误用--tests生产lint的日志保持为scope错误，不虚报绿色或修193个无关历史tests。旧组合23/3447属于内部历史Git snapshot；本 focused 证据采集当时新23仍在执行，旧23不用于本增量完成证明。当前新23已完成，见 evidence.framework-parent-drop-final-verification。

## 已知缺口

独立host_actor_delta_review实际完整源码复审pass/action0，本 focused 证据采集当时父scope仍待新full23/当前strict；这两项现已完成并另归档，SDK最终published pin/消费者/main仍待验收；公共后端跨进程lease不由此伪造。

快照保全：原 b0d162 完整846文件源码已保存为内部 recoverable Git snapshot 68925f49814e50a7074e11cc329c40b00c44d2d1；此历史审查/门禁不证明后续工具校验 public 入口改动，也不是已公开 pin。
