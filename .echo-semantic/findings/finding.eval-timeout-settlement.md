---
schema_version: 1
id: finding.eval-timeout-settlement
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: time_lifecycle
focus: [failure_concurrency, result_side_effect]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution, behavior.agent-turn-lifecycle]
rule_refs: [rule.quality-observation-boundary, rule.turn-terminal-authority]
evidence_refs: [evidence.provider-protocol-quality, evidence.agent-context-execution]
audit_refs: [audit.eval-evolution.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Eval timeout 取消后未等待 Turn settlement

## 问题

Eval timeout 只触发 cancellation token 后立即评分，Agent stream producer 是独立 task，未见等待 receipt/terminal 的边界。

## 触发条件与影响

Timeout 后 tool/file effect 可能继续运行并污染 fixture，评分与 cleanup 同时发生，后续 case 看到非终态状态。

## 证据

`src/eval/runner.rs` 与 `src/agent/react/run/stream_channel.rs` 的 timeout/spawn 路径提供证据。

## 处理记录

Discovery 记录；下一阶段让 Eval 消费 bounded Turn settlement 后再评分/清理。
