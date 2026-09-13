---
schema_version: 1
id: finding.eval-timeout-settlement
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: time_lifecycle
focus: [failure_concurrency, result_side_effect]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution, behavior.agent-turn-lifecycle]
rule_refs: [rule.quality-observation-boundary, rule.turn-terminal-authority]
evidence_refs: [evidence.provider-protocol-quality, evidence.agent-context-execution, evidence.eval-workspace-generation-verification, evidence.eval-timeout-turn-settlement-repair, evidence.eval-timeout-turn-settlement-verification]
audit_refs: [audit.eval-evolution.failure-concurrency, audit.eval-timeout-turn-settlement-rereview]
decision_refs: []
repair_evidence_refs: [evidence.eval-timeout-turn-settlement-repair]
verification_evidence_refs: [evidence.eval-timeout-turn-settlement-verification]
rereview_audit_refs: [audit.eval-timeout-turn-settlement-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Eval timeout 取消后未等待 Turn settlement

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/48

## 问题

Eval timeout 只触发 cancellation token 后立即评分，Agent stream producer 是独立 task，未见等待 receipt/terminal 的边界。

## 触发条件与影响

Timeout 后 tool/file effect 可能继续运行并污染 fixture，评分与 cleanup 同时发生，后续 case 看到非终态状态。

## 证据

`src/eval/runner.rs` 与 `src/agent/react/run/stream_channel.rs` 的 timeout/spawn 路径提供证据。

## 处理记录

Issue #48追踪。Eval已复用AgentTurnDriver并在deadline后对同一drive future等待共享bounded grace；React terminal不领先于自有producer settlement；收到receipt才读取terminal trace和close workspace，未settled则跳过评分并retain。Repair、verification与三轮独立复审已闭合本Finding。GitHub Issue保持open，等待远端main交付。
