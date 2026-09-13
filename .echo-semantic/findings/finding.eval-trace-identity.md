---
schema_version: 1
id: finding.eval-trace-identity
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [contract_evidence, result_side_effect]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution, behavior.observation-persistence]
rule_refs: [rule.quality-observation-boundary, rule.fact-projection-separation]
evidence_refs: [evidence.provider-protocol-quality, evidence.persistence-observation]
audit_refs: [audit.observation-persistence-delivery.state-authority]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
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

Discovery 记录；下一阶段由 execution result/receipt 显式返回 trace identity，不解析 product ID。
