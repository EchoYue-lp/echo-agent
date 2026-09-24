---
schema_version: 1
id: finding.plan-mode-write-surface
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: permission_external
focus: [result_side_effect, state_authority, contract_evidence]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution]
rule_refs: [rule.permission-effect-order]
evidence_refs: [evidence.effects-extensions, evidence.plan-mode-write-surface-repair, evidence.plan-mode-write-surface-timing-verification]
audit_refs: [audit.tool-permission-sandbox.permission-external, audit.plan-mode-write-surface-timing]
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
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
该能力事实。当前主线的既有 Plan 测试 2/2 通过，但定向时序反例在 PreToolUse Hook
等待期间切换 PermissionService 到 Plan，Hook Allow 后仍执行一次 mutation（exit 101）。
执行前缺少对 live Plan 状态的最终检查，因此本 Finding 保持 open，详见新增
timing verification 与 audit。修复与持久红绿回归留给独立实施分支。
