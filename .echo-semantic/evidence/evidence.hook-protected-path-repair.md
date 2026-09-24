---
schema_version: 1
id: evidence.hook-protected-path-repair
kind: evidence
observed_at: source:13ff9de40ae621e1201c111201fda28402a397d7104e90595be0c5106482dbc0
source_refs:
  - echo-orchestration/src/human_loop/service.rs
  - src/agent/react/run/pipeline.rs
  - docs/adr/0071-protected-path-and-readonly-tool-boundary.md
  - docs/en/05-human-loop.md
  - docs/zh/05-human-loop.md
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 只约束 framework Agent 自动工具调用与 PermissionService 审批入口；trusted Hook 自身的 effect 由扩展合同承担
  - 完整 workspace 门禁与 17 项独立 feature 编译由 verification Evidence 记录；PR/CI 与远端 main 交付待完成
---

# Hook protected-path 修复证据

## 支持的结论

`PermissionService::protected_path_decision` 成为受保护路径的单一判断与审计入口。
`PermissionStage` 在 PreToolUse 的有效输入已确定后、任何 Hook Allow 短路前调用它；
PermissionRequest Hook Allow 也只能在该拒绝之后作用于普通路径。服务端审批 handler
返回 `updated_input` 时，同一 checker 检查重写后的输入，拒绝只记一条审计，避免
原输入和有效输入重复记录。

## 来源与范围

修复位于 `cd37e5d3`，ADR 0071 记录复用既有 PermissionService 而不引入第二套
protected-path 判定的原因；双语 Human Loop 文档同步自动工具路径合同。实现涉及
`echo-orchestration/src/human_loop/service.rs` 与 `src/agent/react/run/pipeline.rs`。

## 已知缺口

本证据不声明用户主动的终端/MCP 连接路径获得 Agent 权限门禁，也不证明任意 Hook
command/http 自身的 effect 受 protected-path checker 约束。远端 PR/CI 和 main
交付仍需单独验收。
