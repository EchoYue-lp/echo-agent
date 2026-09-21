---
schema_version: 1
id: finding.checkpoint-current-plan-orphan-authority
kind: finding
type: authority_conflict
status: resolved
severity: medium
primary_focus: data_durability
focus: [state_authority, contract_evidence]
boundary_ref: boundary.context-memory
behavior_refs: [behavior.context-memory-lifecycle, behavior.task-subagent-execution]
rule_refs: [rule.context-persistence-separation, rule.task-subagent-authority]
evidence_refs: [evidence.agent-context-execution, evidence.task-subagent-workflow, evidence.checkpoint-plan-authority-repair, evidence.checkpoint-plan-authority-verification]
audit_refs: [audit.context-memory.data-durability, audit.checkpoint-plan-authority-rereview]
decision_refs: []
repair_evidence_refs: [evidence.checkpoint-plan-authority-repair]
verification_evidence_refs: [evidence.checkpoint-plan-authority-verification]
rereview_audit_refs: [audit.checkpoint-plan-authority-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# AgentCheckpoint current_plan 是孤立恢复权威

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/42

## 问题

`current_plan` 被注释和文档描述为可恢复运行态，但生产代码只有 reset、restore、checkpoint 读取与重写，未发现 canonical Task artifact writer。

## 触发条件与影响

旧 backend 或外部构造的非空值可被恢复并持续重写，却无法证明与 revisioned Task graph/Plan artifact 同步，形成第二或陈旧 plan authority。

## 证据

`src/agent/snapshot.rs`、`src/agent/react/mod.rs` 与 `src/state/mod.rs` 展示 current_plan 的生产读写闭集。

## 处理记录

ADR 0008 选择保留公开字段的历史读写合同，退役 ReAct 私有 plan 状态及恢复/写回路径。
File/SQLite 重启、旧值、受管 transcript 结算、取消 hydration 和 A -> B -> A 隔离
已有 focused 回归；完整门禁、17-feature matrix、strict semantic 与独立复审均通过。
外部 Issue 仍等待 PR CI、远端 main 交付及 post-merge 复验。
