---
schema_version: 1
id: evidence.hook-permission-precedence-repair
kind: evidence
observed_at: 9d9c7a0ae698ba275c349d9331e90182411ef908
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
  - Finding 仍需独立复审和远端主线验证后才能关闭
---

# Hook permission precedence 修复证据

## 支持的结论

`HookAction::Permission` 不再隐式设置 `stop_propagation`。匹配来源继续按确定性顺序
执行，但 `HookResult` 的唯一 reducer 会收集 allow、ask、require_approval 和 deny，按
`deny > ask > require_approval > allow` 归约。显式 `continue: false` 仍能停止传播，deny
仍通过 `block` 结束 Agent 自动工具调用。

## 来源与范围

修复扩展既有 `HookRegistry::run_hooks`/`merge_result` 权威，没有新增权限服务、状态机或
并行 evaluator。PreToolUse/PermissionRequest 的自动工具路径因此不会再被早期
UserConfig allow 或 ask 隐藏的后续 Plugin/Skill deny 绕过。

## 已知缺口

本证据不声明 direct-user adapter、终端、文件选择器、MCP 连接或真实 UI approval provider
的行为；这些边界不属于本 Finding 的 framework 修复范围。
