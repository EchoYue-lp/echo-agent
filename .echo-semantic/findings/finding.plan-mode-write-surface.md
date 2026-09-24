---
schema_version: 1
id: finding.plan-mode-write-surface
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: permission_external
focus: [result_side_effect, state_authority, contract_evidence]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.plan-mode-write-surface-repair, evidence.plan-mode-write-surface-timing-verification, evidence.plan-mode-write-surface-closure]
audit_refs: [audit.tool-permission-sandbox.permission-external, audit.plan-mode-write-surface-timing, audit.plan-mode-write-surface-closure]
decision_refs: []
repair_evidence_refs: [evidence.plan-mode-write-surface-repair]
verification_evidence_refs: [evidence.plan-mode-write-surface-timing-verification, evidence.plan-mode-write-surface-closure]
rereview_audit_refs: [audit.plan-mode-write-surface-timing, audit.plan-mode-write-surface-closure]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Plan mode 未形成可靠只读 surface

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/70

## 问题

PlanModeStage 以有限工具名列表拦截写文件/shell/delete，但 git commit/branch/worktree 等 mutation 可穿过；readonly_tools 又是独立的注册时机制，默认 PermissionService 可不存在。

## 触发条件与影响

Agent 开启 plan mode 且拥有其它 mutation tool 时，仍可能改变 repository 或外部状态，违背 plan read-only 合同。

## 证据

`src/agent/react/mod.rs`、`src/agent/react/run/pipeline.rs` 与 `echo-tools/src/registry.rs` 显示三个未统一的 read-only 控制面。

## 处理记录

`3735f7e0` 用 ToolCapabilities 取代工具名列表；`f7c1fef7` 的 readonly Agent 也复用
该能力事实。当前修复在 ExecuteStage 的 effect boundary 重新检查 live Plan，并在
ToolManager 每次物理 attempt 通过 validation、permit、retry delay 后再次执行 admission；
持久回归验证 PreToolUse/Permission 的旧 Allow 不会绕过新 Plan，晚到拒绝保持
blocked/Unavailable 终态且不产生 mutation。独立复审 PASS；PR #159 已合入
`origin/main@a8a4d945`，远端 CI 与完整本地门禁通过，Issue #70 已关闭，交付分支和
worktree 已删除。本 Finding 已满足 resolved 条件。
