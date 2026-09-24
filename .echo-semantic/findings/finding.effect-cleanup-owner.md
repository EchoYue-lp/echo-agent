---
schema_version: 1
id: finding.effect-cleanup-owner
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: time_lifecycle
focus: [failure_concurrency, result_side_effect, state_authority]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.effect-cleanup-owner-repair, evidence.effect-cleanup-owner-verification]
audit_refs: [audit.tool-permission-sandbox.result-side-effect, audit.effect-cleanup-owner-rereview]
decision_refs: []
repair_evidence_refs: [evidence.effect-cleanup-owner-repair]
verification_evidence_refs: [evidence.effect-cleanup-owner-verification]
rereview_audit_refs: [audit.effect-cleanup-owner-rereview]
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

`1f53a12c` 在各资源创建处持有精确身份和清理责任；Artifact 的保留时点仍由宿主
决定，React close 不擅自删除会话产物，框架保证已请求清理的状态可见。Artifact pending/debt、Docker/K8s
实例级 owner、Agent close 与 worktree marker/ack 的专项反例已验证。独立最终复审
PASS、0 findings。最终源码的 `./scripts/verify.sh` 全量本地门禁与独立
17-feature 条件矩阵均 exit 0。本 Finding 只在当前任务分支标记 resolved；
远端 PR/CI、main 交付和 Issue #47 关闭分别验收。K8s caller-drop 的原独立 Finding 已由此前专项闭合，
本轮只处理实例级清理 owner。
