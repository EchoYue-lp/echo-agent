---
{
  "schema_version": 1,
  "id": "finding.subagent-physical-settlement-observation",
  "kind": "finding",
  "type": "implementation_bug",
  "status": "open",
  "severity": "high",
  "primary_focus": "time_lifecycle",
  "focus": [
    "state_authority",
    "failure_concurrency",
    "result_side_effect",
    "contract_evidence"
  ],
  "boundary_ref": "boundary.task-subagent-workflow",
  "behavior_refs": [
    "behavior.task-subagent-execution",
    "behavior.agent-turn-lifecycle"
  ],
  "rule_refs": [
    "rule.task-subagent-authority",
    "rule.turn-terminal-authority"
  ],
  "evidence_refs": [
    "evidence.subagent-physical-settlement-repair"
  ],
  "audit_refs": [
    "audit.subagent-physical-settlement-focused-review"
  ],
  "decision_refs": [],
  "discovered_at": "00782b8aeddefe2ea8c566922cbdee6aedc80d70",
  "repair_evidence_refs": [
    "evidence.subagent-physical-settlement-repair"
  ],
  "verification_evidence_refs": [
    "evidence.subagent-physical-settlement-repair"
  ],
  "rereview_audit_refs": [
    "audit.subagent-physical-settlement-focused-review"
  ]
}
---

# Cancellation observation cannot settle the original physical owner

## 问题

Original grace abort and logical stream terminal/EOF could precede physical
producer cleanup, releasing claim, permit, isolation or invocation guards and
misclassifying observation uncertainty as Failed. Canonical authority remains
the original RuntimeTaskService/RuntimeDagExecutor/SubagentExecutor, original
producer and exact-claim CAS.

## 触发条件与影响

The user selected B, recorded in ADR0088: first admitted cancellation starts only
an observer deadline, normal execution has no grace timer, exact A does not
interrupt sibling B, and unknown closure never authorizes repeated execution.

## 证据

Current source and focused results are bound by the Evidence and independent
Audit. Manager 335, React run 189, Subagent 69 and RunCode 16 passed; formatter and
input guards had no drift. Three Task claim cases retain the original producer,
complete claim and actual terminal outcome; they are not represented as separate
original-source before executions. Final source review is PASS C0/I0/M0 and
supports the original focused development checkpoint. Six local gates have since
passed as recorded below. The Finding stays open for the identified live-source observation-to-terminal
gap and consumer/main delivery.

## 处理记录

Closure requires current-source held-cleanup, normal-long-run, exact sibling,
repeat-request, caller-drop/unwind, unknown restart, late uncertainty, original
mode capacity/resources, input watch and post-CAS priority proofs with independent
review. Focused verification does not represent merged-main completion.

ACP close increment is independently reviewed C0/I0/M0 after genuine held-backend
identity/retry and unidentified owner-loss failures. All 18 original Session tests
pass after the repair, including ordinary retry/success. Remaining full merge,
feature and consumer/main acceptance keeps this Finding open.

最终本地六门禁已全部通过（3580主通过/0失败/3既有live opt-in忽略），Headless便利返回丢唯一owner的真实反例已按同handle投影修复并独审C0/I0/M0；原3fixture错误保持失败记录并以更强原EOF/Unknown断言纠正。此更新不宣独feature或消费者/main通过。Plan验收明确精确merged-main CI，故当前ready与本Finding open保留交付缺口；后续只按真实结果收口。

当前23适用本地门禁实际0（6原命令+17独feature），但已识别live source上的观察知识会在后续合法terminal+EOF后仍被无条件上送，原owner可能无法结算。后继尚未新增真实red/green或改源；无source Unknown规则不能覆盖有同owner/gen真实结算来源的场景。本checkpoint保留该明确缺口，未宣全B完成，未合main。

## Live-source后继的局部完成

f0a6公开checkpoint之后，同源真实terminal+physical EOF仍被旧观察覆盖的Driver反例已实际101并修后0；Core426/Orch477/Subagent70及适用三crate Clippy/fmt实际0，独立有限source增量C0/I0/M0。匹配源成功/取消结算、无terminalEOF、foreign/wrong-attempt、晚观察和sink独立强例保留。旧23资格仍固定f0a6，不借给当前新源码。

本Finding继续open：已确认Scheduler callback把typed观察变Failed/终态及owner-loss重投，Workflow resume无条件requeue且heartbeat失败会丢原future，File lease或远端checkpoint恢复尚无未知物理权威闭包。两者按原Plan同Outcome后继独立调查/真实counter/窄设计；未实施，不声称新恢复协议或普通重试合同全改。当前全6+17与跨仓消费、精确merged-main CI仍待最终代码验证。
