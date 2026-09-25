---
schema_version: 1
id: audit.approval-authority-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: permission_external
freshness: examined
revision: source:33686a6a0273c6f8ea85608bff92fed9774f00bfefe1bbd34aa1a2316b236cbc
finding_refs: [finding.approval-authority]
challenges:
  permission-request-rewrite-order:
    revision: source:33686a6a0273c6f8ea85608bff92fed9774f00bfefe1bbd34aa1a2316b236cbc
    source_refs: [src/agent/react/run/pipeline.rs, src/agent/snapshot.rs]
    evidence_refs: [evidence.approval-authority-repair, evidence.approval-authority-verification]
  exact-effect-receipt:
    revision: source:33686a6a0273c6f8ea85608bff92fed9774f00bfefe1bbd34aa1a2316b236cbc
    source_refs: [echo-core/src/tools/permission.rs, echo-core/src/tools/mod.rs, echo-tools/src/shell.rs]
    evidence_refs: [evidence.approval-authority-repair, evidence.approval-authority-verification]
---

# Permission receipt authority 独立复审

## 审查范围

Independent reviewer 复核了 PermissionService、PreToolUse 与 PermissionRequest
Hook rewrite 顺序、ToolContext receipt 传递，以及 ShellTool foreground、background
和 streaming effect boundary。复审快照为 framework-only Issue #37 候选；EKO 应用
provider、完整 workspace gate、远端 CI 与 main 交付不在本次 review 的证明范围内。

## 已检查故障假设

- PermissionRequest Hook 的 `updated_input` 可能在 Allow receipt 签发后才应用，导致
  receipt 绑定旧参数。
- 一次 PermissionService/Hook Allow 可能无法穿过 ToolContext 到达真实 Shell effect。
- background 或 streaming 路径可能绕过 foreground 的 receipt 检查；Dangerous 或
  不带 receipt 的 direct ToolManager 调用可能被错误放行。

## 实际复审结论

实现 PASS，无 Critical、Important 或 Minor blocker。PermissionRequest rewrite 先
进入 pipeline effective input，再执行 protected-path 检查和 Allow receipt 签发；真实
PermissionRequest Hook rewrite + Allow → Shell `python3 --version` 回归通过。Shell 的
三类执行路径共用精确 tool name + canonical args 检查，Dangerous 仍拒绝，缺失 receipt
仍失败。

## 验证范围

Focused tests、受影响 package Clippy（普通与 panic-policy）、formatter 与 diff check
均通过。完整 workspace gate、17-feature matrix、PR/CI 和 main 交付仍由 delivery gate
负责。

## 残余风险

`ToolApprovalReceipt::issue` 是公开的 caller-owned transport constructor；框架不把它
当作持久化授权或第二规则注册表。调用方仍需将 receipt 绑定到当前 invocation，跨进程
恢复必须重新经过 PermissionService/宿主授权。

## 实际实现路径与证据

PermissionStage 应用最终 hook rewrite 后，PermissionService/Hook Allow 产生
ToolApprovalReceipt；pipeline 将其注入 ToolContext，Shell 的三条 effect 路径执行
精确匹配。focused real-caller tests 覆盖 PermissionRequest rewrite、background 无
receipt 和 Dangerous/direct ToolManager 拒绝。

## 问题记录

复审未发现新的 framework blocker。

## 未检查项

未执行完整 workspace gate、17-feature matrix、远端 CI 或应用层 consumer 验证。
