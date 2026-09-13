---
schema_version: 1
id: finding.tool-terminal-observation-divergence
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: result_side_effect
focus: [state_authority, contract_evidence, data_durability]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution, behavior.observation-persistence]
rule_refs: [rule.permission-effect-order, rule.fact-projection-separation]
evidence_refs: [evidence.effects-extensions, evidence.persistence-observation]
audit_refs: [audit.tool-permission-sandbox.result-side-effect]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Tool caller、trace 与 audit 可记录不同终态

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/102

## 问题

PostToolUse block 在 effect 后短路 OutputGuard/artifact/Trace/CallbackEnd；失败 ToolResult 仍调用 on_tool_end，on_tool_error 无生产调用，AuditCallback 可把失败记录成成功。

## 触发条件与影响

同一次已发生外部 effect 可让 caller 收到 block/failure、trace 只有 ToolCall、audit 记录 success，破坏观察与恢复判断。

## 证据

`src/agent/react/run/pipeline.rs` 的 stage 顺序、post-hook short-circuit 与 callback调用构成生产反例。

## 处理记录

Result-side-effect Audit 确认；后续 repair 需统一 typed terminal observation，并明确 post-effect policy failure 与 effect outcome 分离。
