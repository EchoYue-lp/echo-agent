---
schema_version: 1
id: finding.a2a-stream-cleanup
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, result_side_effect, state_authority]
boundary_ref: boundary.protocol-surfaces
behavior_refs: [behavior.protocol-projection]
rule_refs: [rule.protocol-role-separation]
evidence_refs: [evidence.provider-protocol-quality]
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# A2A streaming cancel 与 cleanup 未闭合

## 问题

Streaming 路径使用 execute_stream 并只在下一个 event 到达后检查 cancel；setup/event error 未始终移除 cancel token，消费者提前 drop 也无 RAII settlement。

## 触发条件与影响

Provider stall、stream setup failure、event error 或 client disconnect 时，A2A task/cancel entry 与底层执行可能在终态后继续存在。

## 证据

`src/a2a/server.rs` 的 streaming loop、error arms 和 cancel_tokens cleanup 提供源码证据。

## 处理记录

Discovery 记录；下一阶段与 TurnDriver 统一 cancel/terminal/cleanup owner。
