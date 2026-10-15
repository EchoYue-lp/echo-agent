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
supports a focused development checkpoint. The Finding stays open for remaining
acceptance, including six gates, applicable features and consumer/main delivery.

## 处理记录

Closure requires current-source held-cleanup, normal-long-run, exact sibling,
repeat-request, caller-drop/unwind, unknown restart, late uncertainty, original
mode capacity/resources, input watch and post-CAS priority proofs with independent
review. Focused verification does not represent merged-main completion.

ACP close increment is independently reviewed C0/I0/M0 after genuine held-backend
identity/retry and unidentified owner-loss failures. All 18 original Session tests
pass after the repair, including ordinary retry/success. Remaining full merge,
feature and consumer/main acceptance keeps this Finding open.
