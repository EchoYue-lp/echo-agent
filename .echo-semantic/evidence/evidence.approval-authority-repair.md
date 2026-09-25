---
schema_version: 1
id: evidence.approval-authority-repair
kind: evidence
observed_at: source:623cac6a21b2ee853131bb0849f3228afdbfe78ec9c4eec731be622d4322d6b6
source_refs:
  - echo-core/src/tools/permission.rs
  - echo-core/src/tools/mod.rs
  - echo-orchestration/src/human_loop/service.rs
  - src/agent/snapshot.rs
  - src/agent/react/run/pipeline.rs
  - echo-tools/src/shell.rs
  - docs/adr/0075-invocation-approval-receipt.md
supports: [finding.approval-authority, behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 本证据绑定当前 repair branch，尚未证明远端 PR/CI、main 交付或 Issue 关闭
  - 仅覆盖 framework PermissionService、React pipeline 与 ShellTool；EKO 应用层 approval provider 仍由调用方拥有
  - receipt 是 invocation transport，不是跨进程持久化授权；进程重启后的审批需要重新由宿主签发
---

# Issue 37 PermissionService 与 Shell CommandPolicy 修复证据

## 支持的结论

PermissionService 和显式 Hook Allow 在所有参数 rewrite 完成后产生
`ToolApprovalReceipt`。receipt 绑定最终 tool name 与递归 canonical JSON
arguments，随后通过 `ToolContext` 进入 ToolManager 的 effect boundary。
ShellTool 的 foreground、background-cell 与 streaming 路径共用精确匹配；无
receipt 的 direct ToolManager 调用仍拒绝 `RequiresApproval`，`Dangerous` 始终拒绝。

## 来源与范围

实现涉及 `echo-core` 的 receipt/context 合同、`echo-orchestration` 的
PermissionService、React Agent pipeline 的 rewrite/permission 顺序，以及
`echo-tools` ShellTool 的三类 effect boundary；不引入 EKO 应用状态或持久化授权。

## 已检查路径

- PermissionService handler `updated_input` 先成为 effective input，再签发 receipt。
- React PermissionStage 在 PreToolUse/PermissionRequest Hook Allow 及 PermissionService
  Allow 两条路径传递 receipt，并校验 receipt 与 pipeline 当前 input 一致。
- ShellTool 在命令分类后、进程/命令 cell 启动前消费 receipt；安全命令不额外要求 receipt。

## 已知缺口

完整 workspace gate、17-feature public API matrix、远端 CI、独立复审和 main 交付尚未执行；
这些不是本 repair evidence 的结论。
