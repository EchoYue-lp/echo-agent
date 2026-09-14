---
schema_version: 1
id: finding.eval-trace-identity
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: state_authority
focus: [contract_evidence, result_side_effect]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution, behavior.observation-persistence]
rule_refs: [rule.quality-observation-boundary, rule.fact-projection-separation]
evidence_refs: [evidence.provider-protocol-quality, evidence.persistence-observation, evidence.eval-trace-correlation-repair, evidence.eval-trace-correlation-verification]
audit_refs: [audit.observation-persistence-delivery.state-authority, audit.eval-trace-correlation-rereview]
decision_refs: []
repair_evidence_refs: [evidence.eval-trace-correlation-repair]
verification_evidence_refs: [evidence.eval-trace-correlation-verification]
rereview_audit_refs: [audit.eval-trace-correlation-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Eval 使用 product run ID 查询 trace

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/49

## 问题

EvalRunner 从 agent.current_run_id 获取 trace link，但 ReactAgent 该值是外部 product run；真实 trace ID 是 invocation 内局部 trace_run_id。

## 触发条件与影响

对 ReactAgent 执行 Eval 时，ToolUsed、metrics 和 trace-based constraints 可能查询不到刚生成的 RunStore 记录。

## 证据

`src/eval/runner.rs`、`src/agent/react/mod.rs` 与 `src/agent/react/run/stream_channel.rs` 显示两个 identity 来源。

## 处理记录

Issue #49追踪。Eval已使用每次invocation唯一run/turn/execution correlation，从RunStore解析并二次验证真实trace Run；EvalResult不再读取Agent product run getter。Repair、verification与独立复审已闭合本Finding。GitHub Issue保持open，等待远端main交付。
