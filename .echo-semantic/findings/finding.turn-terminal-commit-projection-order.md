---
schema_version: 1
id: finding.turn-terminal-commit-projection-order
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: state_authority
focus: [data_durability, time_lifecycle, failure_concurrency]
boundary_ref: boundary.observation-persistence-delivery
behavior_refs: [behavior.agent-turn-lifecycle, behavior.observation-persistence]
rule_refs: [rule.turn-terminal-authority, rule.fact-projection-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation, evidence.turn-terminal-delivery-settlement-repair, evidence.turn-terminal-delivery-settlement-verification]
audit_refs: [audit.observation-persistence-delivery.state-authority, audit.protocol-surfaces.state-authority, audit.protocol-surfaces.time-lifecycle, audit.turn-terminal-delivery-settlement-rereview]
decision_refs: []
repair_evidence_refs: [evidence.turn-terminal-delivery-settlement-repair]
verification_evidence_refs: [evidence.turn-terminal-delivery-settlement-verification]
rereview_audit_refs: [audit.turn-terminal-delivery-settlement-rereview]
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

ADR 0046确认execution/delivery双结果。commit `cba8e08f3e3f0ccf1d4df3a22be11589f63b2ecd`
完成driver、ACP与SDK persistence/recovery收敛；三轮独立复审后的最终结论为pass。
