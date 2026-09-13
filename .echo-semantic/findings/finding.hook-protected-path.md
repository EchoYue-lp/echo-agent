---
schema_version: 1
id: finding.hook-protected-path
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: permission_external
focus: [state_authority, result_side_effect]
boundary_ref: boundary.tool-permission-sandbox
behavior_refs: [behavior.effect-permission-execution, behavior.extension-publication]
rule_refs: [rule.permission-effect-order, rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# Hook Allow 可绕过 protected-path decision

## 问题

PreToolUse/PermissionRequest Hook 返回 Allow 时 PermissionStage 立即返回，不再调用 PermissionService，因而绕过其文档化最高优先级 protected path 检查。

## 触发条件与影响

匹配 Hook 对文件 mutation 给出 Allow 时，Agent 自动路径可能执行 PermissionService 本应拒绝的受保护路径操作。

## 证据

`src/agent/react/run/pipeline.rs` 的 Hook short-circuit 与 `echo-orchestration/src/human_loop/service.rs` 的 protected-path 顺序构成反例。

## 处理记录

Discovery 记录；下一阶段需决定不可绕过的本地数据保护位置，并补 Hook+Permission 组合测试。
