---
schema_version: 1
id: finding.guard-direction-contract
kind: finding
type: intent_gap
status: open
severity: medium
primary_focus: contract_evidence
focus: [failure_concurrency, result_side_effect]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.tool-permission-sandbox.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Guard ToolInput/ToolOutput 与生产可达性错位

## 问题

GuardDirection 定义 ToolInput/ToolOutput，但未发现生产调用；工具输出使用通用 Output，单 Guard error 又被降级为 Warn，使上层 fail-closed 分支难以到达。

## 触发条件与影响

Consumer 配置 tool-specific guard 或依赖 guard error 阻断时，实际行为可能不同于公共合同。

## 证据

`echo-core/src/guard/mod.rs` 与 `src/agent/snapshot.rs` 的 direction/error handling 提供证据。

## 处理记录

Discovery 记录；下一阶段确认应接通、退役或重命名这些 direction，并统一 error policy。
