---
schema_version: 1
id: finding.trace-effect-event-producers
kind: finding
type: evidence_gap
status: open
severity: medium
primary_focus: contract_evidence
focus: [result_side_effect, data_durability]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution, behavior.observation-persistence]
rule_refs: [rule.permission-effect-order, rule.fact-projection-separation]
evidence_refs: [evidence.effects-extensions, evidence.persistence-observation]
audit_refs: [audit.observation-persistence-delivery.state-authority, audit.observation-persistence-delivery.contract-evidence, audit.tool-permission-sandbox.result-side-effect]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Trace Permission/File/Test 事件缺少生产点

## 问题

RunEvent 定义并在文档承诺 PermissionDecision、FileEdit、TestRun、Error、SubagentRun，但全仓未发现对应构造点；文档称 11 类而源码有 14 类，permission audit 另以 fire-and-forget 写入。

## 触发条件与影响

Eval、Analyzer 或用户依赖 trace 判断 permission/file/test 行为时，可能获得永久缺失的事件。

## 证据

`src/trace/mod.rs`、`docs/en/27-tracing.md` 与全仓构造点搜索提供证据。

## 处理记录

Discovery 记录；下一阶段确认这些是应接通的合同还是应退役的未实现 API。
