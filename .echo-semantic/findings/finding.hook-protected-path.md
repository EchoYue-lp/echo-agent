---
schema_version: 1
id: finding.hook-protected-path
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: permission_external
focus: [state_authority, result_side_effect]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution, behavior.extension-publication]
rule_refs: [rule.permission-effect-order, rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions, evidence.hook-protected-path-repair, evidence.hook-protected-path-verification]
audit_refs: [audit.tool-permission-sandbox.permission-external, audit.extension-lifecycle.permission-external, audit.hook-protected-path-rereview]
decision_refs: []
repair_evidence_refs: [evidence.hook-protected-path-repair]
verification_evidence_refs: [evidence.hook-protected-path-verification]
rereview_audit_refs: [audit.hook-protected-path-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Hook Allow 可绕过 protected-path decision

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/60

## 问题

PreToolUse/PermissionRequest Hook 返回 Allow 时 PermissionStage 立即返回，不再调用 PermissionService，因而绕过其文档化最高优先级 protected path 检查。

## 触发条件与影响

匹配 Hook 对文件 mutation 给出 Allow 时，Agent 自动路径可能执行 PermissionService 本应拒绝的受保护路径操作。

## 证据

`src/agent/react/run/pipeline.rs` 的 Hook short-circuit 与 `echo-orchestration/src/human_loop/service.rs` 的 protected-path 顺序构成反例。

## 处理记录

`cd37e5d3` 复用 PermissionService 的 protected-path 决策，使 PreToolUse 和
PermissionRequest Hook Allow 均不能越过有效输入的受保护路径拒绝；handler 重写输入也
经同一检查并只产生一次审计。focused 反例、独立复审分别见上述 verification 与 rereview
引用。当前任务分支完整门禁与 17 项独立 feature 编译已通过；PR/CI 与远端 main
交付仍待完成。
