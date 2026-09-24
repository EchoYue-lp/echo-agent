---
schema_version: 1
id: audit.plan-mode-write-surface-timing
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: permission_external
freshness: examined
revision: bd17c73075d6b3cf8e00877fa0fb10d36694ea54
finding_refs: [finding.plan-mode-write-surface]
challenges:
  live-plan-switch-after-gate:
    revision: bd17c73075d6b3cf8e00877fa0fb10d36694ea54
    source_refs: [src/agent/react/run/pipeline.rs, src/agent/snapshot.rs, echo-orchestration/src/human_loop/service.rs]
    evidence_refs: [evidence.plan-mode-write-surface-repair, evidence.plan-mode-write-surface-timing-verification]
---

# Plan mode live-switch timing audit

## 审查范围

检查 `origin/main@f7c1fef7` 的 Agent 自动工具调用：ToolRuntime 可见性、
PlanModeStage、PreToolUse Hook、PermissionStage 和 ExecuteStage。此审计记录本轮作者的
定向反例；独立 reviewer 已复核证据与当前源码，但本审计仍不构成修复复审。

## 已检查故障假设

假设 mutating Tool 在 PlanModeStage 看到 Default 后等待 Hook，此时 PermissionService
变为 Plan；Hook Allow 短路后续审批，Tool 仍会执行。

## 实际实现路径与证据

临时同步测试把调用停在 PreToolUse Hook 中，等待 `set_mode(Plan)` 完成后返回
`HookResult::allow()`。测试两次 exit 101，最终一次执行计数为 1 而预期为 0；
原有 Plan 测试 2/2 通过。完整命令、计数和未提交测试位置见
`evidence.plan-mode-write-surface-timing-verification`。

## 问题记录

此反例证明已有 capability 分类仅在 Plan gate 运行时生效，后续 live 模式切换未在
effect 前重新判定。Finding #70 保持 open；本审计不构成修复复审或关闭收据。

## 残余风险

还需确定模式切换对在途调用的精确合同，然后在实施分支加入持久红绿回归和修复。

## 未检查项

独立 reviewer 未自行运行测试；主任务的隔离 target 完整本地门禁已通过，但未包含临时红测。
未执行远端 CI 或第三方 Tool capability 真实性抽样。
