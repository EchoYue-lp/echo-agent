---
{
  "schema_version": 1,
  "id": "evidence.hook-permission-precedence-verification",
  "kind": "evidence",
  "observed_at": "7110a27e7b0e9841dcfa0d4cca1cdbc053d96177",
  "source_refs": [
    "echo-execution/src/skills/hooks.rs",
    "docs/en/07-skills.md",
    "docs/en/23-hooks.md",
    "docs/zh/07-skills.md",
    "docs/zh/23-hooks.md"
  ],
  "supports": [
    "behavior.effect-permission-execution",
    "rule.permission-effect-order"
  ],
  "limitations": [
    "Current7110 native observation only; original unavailable history, old executions and old reviews are not reconstructed or reexecuted.",
    "Current source/test/limit mapping is in evidence.framework-history-current-reobservation; full05 is existing execution, not a newly run historical command."
  ]
}
---

# Current observation and historical record

The machine header now records only the bounded native observation at `7110a27e7b0e9841dcfa0d4cca1cdbc053d96177`; current source/tests and limitations are in `evidence.framework-history-current-reobservation`. Original observation `eadf1a3d5a498bdbccd3742a7e1a457cb27b172d` and its complete metadata remain in [the immutable875 document](https://github.com/EchoYue-lp/echo-agent/blob/875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c/.echo-semantic/evidence/evidence.hook-permission-precedence-verification.md). The original execution counts, before/after claims and review prose below are preserved verbatim as historical statements, not rerun or equivalence claims for7110.


# Hook permission precedence 验证证据

## 支持的结论

- `cargo test -p echo_execution permission`：12 passed，0 failed。
- `permission_output_stop_cannot_hide_later_deny` 覆盖 allow/ask 输出携带
  `continue: false` 后，reducer 仍接受后续 deny，并确认非 permission 输出仍可停止传播。
- `command_permission_stop_cannot_hide_later_source_deny` 覆盖真实 command Hook 先返回
  allow/ask 与 `continue: false`、后续 Skill 来源 deny 仍获胜并阻断调用。
- 既有跨来源组合测试继续证明 UserConfig allow、Plugin ask、Skill deny 归约为 deny。
- `cargo fmt --all -- --check`：通过。
- `cargo clippy -p echo_execution --lib --locked -- -D warnings`：通过。
- `git diff --check`：通过。
- 独立 reviewer 对提交 `eadf1a3d` 复审，Critical、Important、Minor 均为 0，结论 PASS。

## 来源与范围

验证覆盖 `HookRegistry::run_hooks`、`HookAction::Permission`、`parse_hook_output` 与唯一
`merge_result` reducer、真实 command 执行、跨 UserConfig/Plugin/Skill 组合路径，以及同步
更新的中英文 Hook 文档。HTTP 与 command 共享同一输出解析和归约入口。

## 已知缺口

语义 strict snapshot 因共享 `.echo-semantic` source digest 仍绑定旧集成 revision 而报告
漂移；本并行分支按约定不刷新共享 baseline/digest。汇总分支需在组合所有并行提交后统一
刷新并运行完整 change-evidence 门禁。
