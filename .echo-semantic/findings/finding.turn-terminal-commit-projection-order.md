---
schema_version: 1
id: finding.turn-terminal-commit-projection-order
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [data_durability, time_lifecycle, failure_concurrency]
boundary_ref: boundary.observation-persistence-delivery
behavior_refs: [behavior.agent-turn-lifecycle, behavior.observation-persistence]
rule_refs: [rule.turn-terminal-authority, rule.fact-projection-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation]
audit_refs: [audit.observation-persistence-delivery.state-authority, audit.protocol-surfaces.state-authority, audit.protocol-surfaces.time-lifecycle]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Turn terminal commit 与 projection 顺序可形成冲突终态

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/108

## 问题

ReactAgent 在 FinalAnswer 交给 TurnDriver sink 前已保存 checkpoint/transcript 并把 trace 标为 Completed；TurnDriver 或 ACP projector/observer 随后失败可返回 Failed，而先前事实/投影仍保留成功终态。

## 触发条件与影响

Terminal sink、ACP projector 或 observer 失败时，RunStore/checkpoint/transcript/ledger 与 TurnReceipt 可同时出现 Completed/FinalAnswer 和 Failed 两种解释。

## 证据

`src/agent/react/run/phases/finalize.rs`、`echo-orchestration/src/runtime/turn_driver.rs`、`src/acp/runtime.rs` 与 `src/trace/mod.rs` 展示提交顺序和 first-terminal merge。

## 处理记录

State-authority Audit 确认；后续设计需指定唯一 terminal commit point，并把 projection/observer failure 作为独立 delivery failure。
