---
schema_version: 1
id: evidence.hook-permission-precedence-verification
kind: evidence
observed_at: 9d9c7a0ae698ba275c349d9331e90182411ef908
source_refs:
  - echo-execution/src/skills/hooks.rs
  - docs/en/07-skills.md
  - docs/en/23-hooks.md
  - docs/zh/07-skills.md
  - docs/zh/23-hooks.md
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 未执行 echo-agent workspace 全量合并门禁或远端 CI
  - semantic strict snapshot 当前被主线已有的 baseline source digest 漂移阻断，需主线维护者刷新基线后复核
  - 未覆盖应用层 direct-user adapter、真实 UI approval provider 和跨进程 Hook 执行环境
---

# Hook permission precedence 验证证据

## 支持的结论

- `cargo test -p echo_execution skills::hooks`：96 passed，0 failed。
- 新增组合测试 `permission_hooks_reduce_across_sources_before_allow_or_ask_short_circuit`：
  UserConfig allow、Plugin ask、Skill deny 同时匹配时，结果为 deny 且保留 deny reason。
- `cargo fmt --all -- --check`：通过。
- `cargo clippy -p echo_execution --all-targets --all-features --locked -- -D warnings`：通过。
- `git diff --check`：通过。

## 来源与范围

验证覆盖 `HookRegistry::run_hooks`、`HookAction::Permission`、现有 `merge_result` reducer、
跨 UserConfig/Plugin/Skill 的组合路径，以及同步更新的中英文 Hook 文档。

## 已知缺口

语义合同检查在当前任务 worktree 报告主线已有 `.echo-semantic` source digest 与当前
`0878a676` 树不一致，并且原有 baseline base revision 仍指向 `d492c676`。这不是本修复
引入的业务失败；应由汇总分支刷新语义快照后重新运行 strict snapshot 和 change-evidence
门禁。
