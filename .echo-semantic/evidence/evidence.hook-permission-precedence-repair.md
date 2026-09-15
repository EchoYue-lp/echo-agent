---
schema_version: 1
id: evidence.hook-permission-precedence-repair
kind: evidence
observed_at: eadf1a3d5a498bdbccd3742a7e1a457cb27b172d
source_refs:
  - echo-execution/src/skills/hooks.rs
  - src/agent/react/run/pipeline.rs
  - docs/en/07-skills.md
  - docs/en/23-hooks.md
  - docs/zh/07-skills.md
  - docs/zh/23-hooks.md
supports: [behavior.effect-permission-execution, behavior.extension-publication, rule.permission-effect-order]
limitations:
  - 本修复只改变 Agent 自动工具 Hook 的 permission action 归约；不改变 direct-user、终端、文件选择器或 MCP 连接入口
  - deny 仍在最高优先级决策已确定后阻断后续 Hook 执行；其它非 permission action 不因本修复获得新的执行保证
  - 完整 workspace 门禁、远端 CI 和主线语义快照刷新由集成分支统一执行
---

# Hook permission precedence 修复证据

## 支持的结论

`HookAction::Permission` 不再隐式设置 `stop_propagation`。匹配来源继续按确定性顺序
执行，但 `HookResult` 的唯一 reducer 会收集 allow、ask、require_approval 和 deny，按
`deny > ask > require_approval > allow` 归约。command、HTTP 或 programmatic Hook 输出
同时携带 permission decision 与 `continue: false` 时，reducer 忽略该 stop 请求并继续收集
后续匹配来源；普通非 permission 结果仍保留显式停止传播语义。deny 最终通过 `block` 结束
Agent 自动工具调用。

## 来源与范围

修复扩展既有 `HookRegistry::run_hooks`/`merge_result` 权威，没有新增权限服务、状态机或
并行 evaluator。PreToolUse/PermissionRequest 的自动工具路径因此不会再被早期
UserConfig allow 或 ask，包括外部 Hook 输出的 `continue: false`，隐藏后续 Plugin/Skill
deny。

## 已知缺口

本证据不声明 direct-user adapter、终端、文件选择器、MCP 连接或真实 UI approval provider
的行为；这些边界不属于本 Finding 的 framework 修复范围。
