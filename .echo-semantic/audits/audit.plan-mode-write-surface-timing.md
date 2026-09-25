---
schema_version: 1
id: audit.plan-mode-write-surface-timing
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: permission_external
freshness: examined
revision: source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe
finding_refs: [finding.plan-mode-write-surface]
challenges:
  live-plan-switch-after-gate:
    revision: source:8aeadcf34eb574d68a0fdd14a61337195d71c9a6b8cc6cc2d5bc387b168de3fe
    source_refs: [src/agent/react/run/pipeline.rs, src/agent/snapshot.rs, echo-orchestration/src/human_loop/service.rs]
    evidence_refs: [evidence.plan-mode-write-surface-repair, evidence.plan-mode-write-surface-timing-verification]
---

# Plan mode live-switch timing audit and repair review

## 审查范围

检查当前修复分支的 Agent 自动工具调用：ToolRuntime 可见性、PlanModeStage、PreToolUse
Hook、PermissionStage 和 ExecuteStage，并对照原始 timing counterexample。

## 已检查故障假设

假设 mutating Tool 在 PlanModeStage 看到 Default 后等待 Hook，此时 PermissionService
变为 Plan；Hook Allow 短路后续审批，Tool 仍会执行。

## 实际实现路径与证据

原始反例在旧主线中观察到执行计数 1。当前持久回归让真实 `PreToolUse` Hook 暂停，
由控制协程切换 live Plan 后再返回 Allow，并通过 `execute_tool_with_policy` 观察最终
拒绝；执行计数为 0，失败类别为 `Unavailable`。独立 ToolManager 回归还持有唯一
并发 permit，确认释放后才运行 admission，拒绝不会触发目标工具。相关 focused tests、
focused Clippy 和 formatter 均通过。

## 问题记录

修复在 effect boundary 重新读取 live capability，消除 stale Allow 绕过 Plan 的路径，
并把晚到拒绝归一为既有 blocked/Unavailable 终态，保留后续 observation stages；Plan
与 readonly Agent 的拒绝分别保留对应的 reason/source。
Finding #70 仍保持 open，等待独立 rereview、PR/CI、remote main 和 post-merge closure。

## 残余风险

模式切换的合同是：尚未进入 ExecuteStage 的调用必须遵守最新 Plan；已经进入工具 future
的调用不回滚已发生的副作用。

## 未检查项

尚未执行完整 workspace 门禁、远端 CI 或第三方 Tool capability 真实性抽样；retry delay
窗口复用同一 checked admission，但没有单独的 delay-specific race fixture。
