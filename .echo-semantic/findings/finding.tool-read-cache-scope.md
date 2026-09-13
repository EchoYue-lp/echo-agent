---
schema_version: 1
id: finding.tool-read-cache-scope
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: state_authority
focus: [failure_concurrency, data_durability, contract_evidence]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.tool-read-cache-authority-repair, evidence.tool-read-cache-authority-verification]
audit_refs: [audit.tool-permission-sandbox.failure-concurrency, audit.tool-read-cache-authority-rereview]
decision_refs: []
repair_evidence_refs: [evidence.tool-read-cache-authority-repair]
verification_evidence_refs: [evidence.tool-read-cache-authority-verification]
rereview_audit_refs: [audit.tool-read-cache-authority-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# ToolManager read cache 缺少 workspace identity

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/30

## 问题

Read cache key 只有 tool name 与 parameters，`ToolContext.working_dir`、conversation/run identity 不参与 key；共享 ToolManager 会跨 invocation/workspace 复用 ReadOnly 结果。

## 触发条件与影响

两个 workspace 使用相同相对路径或相同参数时，后一个调用可能取得前一个 workspace 的陈旧或错误数据。

## 证据

`echo-execution/src/tools.rs` 展示 cache key 构造、ReadOnly 命中与 ToolContext 分离。

## 处理记录

Issue #30追踪。当前key覆盖effective workspace、invocation lineage与完整artifact policy；无稳定cwd时不缓存，Tool registry变化失效同一cache。确定性red/green、Accepted ADR与三轮独立复审已闭合本Finding。
