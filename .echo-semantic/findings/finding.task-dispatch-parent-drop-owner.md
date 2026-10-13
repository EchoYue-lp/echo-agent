---
schema_version: 1
id: finding.task-dispatch-parent-drop-owner
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: time_lifecycle
focus:
- state_authority
- failure_concurrency
- data_durability
boundary_ref: boundary.task-subagent-workflow
behavior_refs:
- behavior.task-subagent-execution
rule_refs:
- rule.task-subagent-authority
evidence_refs:
- evidence.task-dispatch-parent-drop-repair
- evidence.framework-parent-drop-final-verification
- evidence.framework-final-34d5-full-verification
audit_refs:
- audit.framework-combined-parent-drop-review
- audit.framework-parent-drop-rereview
- audit.framework-final-34d5-gate-rereview
decision_refs: []
repair_evidence_refs:
- evidence.task-dispatch-parent-drop-repair
verification_evidence_refs:
- evidence.task-dispatch-parent-drop-repair
- evidence.framework-parent-drop-final-verification
- evidence.framework-final-34d5-full-verification
rereview_audit_refs:
- audit.framework-parent-drop-rereview
- audit.framework-final-34d5-gate-rereview
discovered_at: 8843afd5029afba9be8a6551de4ab0a93951311f
---


> 以下处理文字是历史发现/修复阶段记录（历史审查快照 8843afd5029afba9be8a6551de4ab0a93951311f）；其中 open、门禁进行中和待复审描述已由下方当前结论取代，未完成交付仍单独保留。


# 父 execute future 在 dispatch join 前被取消后失去结算 owner

## 问题

RuntimeDagExecutor 的局部 JoinSet 被父 future drop 时会 abort dispatch，但 claim/watch projection 已 Active，独立 settlement owner 仅 join 后创建。没有 owner 消费 aborted join 或 exact-claim settle，留下 Running/None。

## 触发条件与影响

公有 RuntimeTaskService::execute、Team 与 Host 调用方可取消 outer future；parked dispatch → retain watch → abort parent，可使 watch 永不发布，后续 exact control 只得到 ActiveRequested。现有父取消测试仅 join 后 CAS，未覆盖本窗口。

## 证据

独立组合复审沿实际源码及调用链确认 Important；完整23门禁3447绿未覆盖该窗口。新的确定性真实 dispatch/reservation/claim await 反例与取消安全 owner 修复正在进行。

## 处理记录

保持 open，不用旧门禁或后join guard证明prejoin闭合。确认真实dispatch stopped 后才canonicalsettle；unknowncommit必须保owner并明确AuthorityUnknown，不伪造receipt或覆盖已settled事实。

已修候选与focused反例/独立复审具备，正在运行新full23；保持open至精确最终证据归档，不用历史门禁强行关项。


## 当前处理结论

本地问题当前闭合：新组合 23 门禁、精确源码及独立复审 pass/action0 已归档；历史证据保持各自快照。公开 pin、SDK/CLI/官网与远端 main 为独立交付义务，本地 resolved 不表示它们完成。


## 当前候选最终复审

audit.framework-final-34d5-gate-rereview 将本地此故障的精确源码/既有红绿证据与同source34d5新完整23门禁绑定；独立implementation review PASS/action0。当前可以进入授权framework里程碑交付，SDK/remoteCI/公开pin及精确main仍另验，其它Finding与全risk lenses不随此关闭。
