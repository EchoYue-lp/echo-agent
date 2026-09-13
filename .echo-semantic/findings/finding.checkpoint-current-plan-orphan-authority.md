---
schema_version: 1
id: finding.checkpoint-current-plan-orphan-authority
kind: finding
type: authority_conflict
status: open
severity: medium
primary_focus: data_durability
focus: [state_authority, contract_evidence]
boundary_ref: boundary.context-memory
behavior_refs: [behavior.context-memory-lifecycle, behavior.task-subagent-execution]
rule_refs: [rule.context-persistence-separation, rule.task-subagent-authority]
evidence_refs: [evidence.agent-context-execution, evidence.task-subagent-workflow]
audit_refs: [audit.context-memory.data-durability]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# AgentCheckpoint current_plan 是孤立恢复权威

## 问题

`current_plan` 被注释和文档描述为可恢复运行态，但生产代码只有 reset、restore、checkpoint 读取与重写，未发现 canonical Task artifact writer。

## 触发条件与影响

旧 backend 或外部构造的非空值可被恢复并持续重写，却无法证明与 revisioned Task graph/Plan artifact 同步，形成第二或陈旧 plan authority。

## 证据

`src/agent/snapshot.rs`、`src/agent/react/mod.rs` 与 `src/state/mod.rs` 展示 current_plan 的生产读写闭集。

## 处理记录

Data-durability Audit 确认孤立性；后续 consolidation/decision 需选择接通 canonical Task artifact 或退役字段。
