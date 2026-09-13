---
schema_version: 1
id: finding.tool-read-cache-scope
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: state_authority
focus: [failure_concurrency, data_durability, contract_evidence]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: []
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.tool-permission-sandbox.failure-concurrency]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# ToolManager read cache 缺少 workspace identity

## 问题

Read cache key 只有 tool name 与 parameters，`ToolContext.working_dir`、conversation/run identity 不参与 key；共享 ToolManager 会跨 invocation/workspace 复用 ReadOnly 结果。

## 触发条件与影响

两个 workspace 使用相同相对路径或相同参数时，后一个调用可能取得前一个 workspace 的陈旧或错误数据。

## 证据

`echo-execution/src/tools.rs` 展示 cache key 构造、ReadOnly 命中与 ToolContext 分离。

## 处理记录

Discovery 记录；后续 audit 决定 cache key scope、per-invocation namespace 或禁用条件。
