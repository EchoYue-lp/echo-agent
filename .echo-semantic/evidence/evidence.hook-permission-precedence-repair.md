---
{
  "schema_version": 1,
  "id": "evidence.hook-permission-precedence-repair",
  "kind": "evidence",
  "observed_at": "7110a27e7b0e9841dcfa0d4cca1cdbc053d96177",
  "source_refs": [
    "echo-execution/src/skills/hooks.rs",
    "src/agent/react/run/pipeline.rs",
    "docs/en/07-skills.md",
    "docs/en/23-hooks.md",
    "docs/zh/07-skills.md",
    "docs/zh/23-hooks.md"
  ],
  "supports": [
    "behavior.effect-permission-execution",
    "behavior.extension-publication",
    "rule.permission-effect-order"
  ],
  "limitations": [
    "Current7110 native observation only; original unavailable history, old executions and old reviews are not reconstructed or reexecuted.",
    "Current source/test/limit mapping is in evidence.framework-history-current-reobservation; full05 is existing execution, not a newly run historical command."
  ]
}
---

# Current observation and historical record

The machine header now records only the bounded native observation at `7110a27e7b0e9841dcfa0d4cca1cdbc053d96177`; current source/tests and limitations are in `evidence.framework-history-current-reobservation`. Original observation `eadf1a3d5a498bdbccd3742a7e1a457cb27b172d` and its complete metadata remain in [the immutable875 document](https://github.com/EchoYue-lp/echo-agent/blob/875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c/.echo-semantic/evidence/evidence.hook-permission-precedence-repair.md). The original execution counts, before/after claims and review prose below are preserved verbatim as historical statements, not rerun or equivalence claims for7110.


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
