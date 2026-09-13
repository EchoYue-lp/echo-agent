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
evidence_refs: [evidence.effects-extensions]
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Plan mode 未形成可靠只读 surface

## 问题

PlanModeStage 以有限工具名列表拦截写文件/shell/delete，但 git commit/branch/worktree 等 mutation 可穿过；readonly_tools 又是独立的注册时机制，默认 PermissionService 可不存在。

## 触发条件与影响

Agent 开启 plan mode 且拥有其它 mutation tool 时，仍可能改变 repository 或外部状态，违背 plan read-only 合同。

## 证据

`src/agent/react/mod.rs`、`src/agent/react/run/pipeline.rs` 与 `echo-tools/src/registry.rs` 显示三个未统一的 read-only 控制面。

## 处理记录

Discovery 记录；下一阶段建立基于 ToolPermission/side-effect 的单一 invocation policy，而非继续扩工具名列表。
