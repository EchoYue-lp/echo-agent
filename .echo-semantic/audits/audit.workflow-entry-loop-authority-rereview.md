---
schema_version: 1
id: audit.workflow-entry-loop-authority-rereview
kind: audit
boundary_ref: boundary.task-subagent-workflow
lens: failure_concurrency
freshness: examined
revision: c7e54e6785f85422af83d4b384f01c2493eddc79
finding_refs: [finding.workflow-entry-loop-drift]
challenges:
  four-entry-single-authority:
    revision: c7e54e6785f85422af83d4b384f01c2493eddc79
    source_refs: [echo-orchestration/src/workflow/graph.rs, docs/adr/0052-workflow-entry-loop-authority.md]
    evidence_refs: [evidence.workflow-entry-loop-authority-repair, evidence.workflow-entry-loop-authority-verification]
  event-terminal-order:
    revision: c7e54e6785f85422af83d4b384f01c2493eddc79
    source_refs: [echo-orchestration/src/workflow/graph.rs, echo-orchestration/src/workflow/node.rs, echo-orchestration/src/workflow/mod.rs]
    evidence_refs: [evidence.workflow-entry-loop-authority-repair, evidence.workflow-entry-loop-authority-verification]
  producer-cancellation-settlement:
    revision: c7e54e6785f85422af83d4b384f01c2493eddc79
    source_refs: [echo-orchestration/src/workflow/graph.rs, echo-orchestration/src/workflow/node.rs]
    evidence_refs: [evidence.workflow-entry-loop-authority-repair, evidence.workflow-entry-loop-authority-verification]
  checkpoint-resume-continuity:
    revision: c7e54e6785f85422af83d4b384f01c2493eddc79
    source_refs: [echo-orchestration/src/workflow/graph.rs, echo-orchestration/src/workflow/checkpoint_store.rs]
    evidence_refs: [evidence.workflow-entry-loop-authority-repair, evidence.workflow-entry-loop-authority-verification]
---

# Workflow 入口循环权威独立复审

## 审查范围

独立reviewer检查Issue #112最终diff、ADR 0052、Graph四个公开入口、Node Agent stream、
fan-out、event channel、interrupt checkpoint与claim/heartbeat/ack/requeue wrapper，并核对
Workflow定向测试、example contract、check、Clippy与formatter证据。

## 已检查故障假设

检查公开入口是否仍复制节点循环，finish before/after interrupt与resume skip是否漂移，fan-out
state/path/step是否失序，`NodeError`是否晚于terminal Err或失败后仍发`Completed`，Token与
FinalAnswer是否在错误提交点投影，以及stream drop、Graph cancel、node timeout和parallel
sibling failure后Agent producer是否脱离继续产生副作用。

## 实际实现路径与证据

首轮review确认路由、事件、finish interrupt、checkpoint continuation与SDK签名收敛，但发现
非流式三个入口仍调用buffered `Agent::execute`，ReactAgent内部task可能在外层future drop后
detach，形成1个Important。修复后Graph四入口统一排空带CancellationToken的Agent stream，
drop guard覆盖四类取消源，旧Node buffered路径退出；70项Workflow与两个example contract、
check、两层Clippy及fmt全部通过。

## 问题记录

最终独立复审结论PASS；Critical 0、Important 0、Minor 0。首轮Important已由四类producer
取消回归闭合，没有发现#112范围内的新Finding。

## 残余风险

完整workspace/all-features、远端Linux/Windows CI及真实provider不可取消外部effect不属于
本次focused证据。共享semantic source digest由integration branch统一刷新；在final gate前
`finding.workflow-entry-loop-drift`与GitHub Issue #112继续保持open。

## 未检查项

未运行完整workspace门禁、逐feature矩阵、远端CI、真实provider网络故障或跨进程恢复。
本Audit不关闭Task DAG/DagWorkflow consolidation及其它Workflow Finding。
