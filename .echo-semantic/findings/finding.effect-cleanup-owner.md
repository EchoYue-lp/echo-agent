---
schema_version: 1
id: finding.effect-cleanup-owner
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: time_lifecycle
focus: [failure_concurrency, result_side_effect, state_authority]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions]
audit_refs: [audit.tool-permission-sandbox.result-side-effect]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Artifact、Sandbox 与 Worktree cleanup owner 未闭合

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/47

## 问题

Artifact scope cleanup 由宿主负责但 React 路径未调用；SandboxManager cleanup 无生产调用点；worktree 创建后 marker 写失败缺少补偿删除。K8s Pod cleanup 的独立 owner/settlement 缺口由专项 Finding 跟踪。

## 触发条件与影响

长期 Agent、shutdown 或 partial worktree creation 时，artifact、sandbox resource 或未标记 worktree 可能无法被可靠回收。

## 证据

`echo-core/src/tools/artifact.rs`、`echo-execution/src/sandbox/manager.rs` 与 `echo-tools/src/git_worktree.rs` 展示当前 owner gap。

## 处理记录

Discovery 记录；K8s caller-drop 仍作为 unresolved 等待故障注入，避免未经运行证据直接下结论。
