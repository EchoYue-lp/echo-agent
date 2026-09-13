---
schema_version: 1
id: finding.subagent-factory-publication-race
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: failure_concurrency
focus: [state_authority, time_lifecycle, contract_evidence]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow]
audit_refs: [audit.task-subagent-workflow.state-authority]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Subagent lazy factory publication 存在双创建窗口

## 问题

Factory 完成后先从 `instantiating` 删除名称，再取得 state lock 发布 agent；第二调用可在两步之间观察“无 agent、无 instantiating”并启动同 revision 的第二次 factory。

## 触发条件与影响

并发 resolve 恰好落在 publication gap 时，同一 Subagent definition 可创建两个实例并竞争发布，违反防 double-creation 的 registry authority。

## 证据

`src/agent/subagent/registry.rs` 的 factory await、标记删除和 state publication 顺序构成源码反例。

## 处理记录

State-authority Audit 确认；可与 cancellation Finding 共用取消安全 publication guard，但需独立确定性交错测试。
