---
schema_version: 1
id: evidence.plugin-lifecycle-reconcile-verification
kind: evidence
observed_at: source:731a8b0e1f5e135908f3e8ea1d7ddaf76c90b108fd0eb3e9b59235efe590501f
source_refs:
  - echo-core/src/plugin/lifecycle.rs
  - docs/adr/0060-plugin-lifecycle-reconcile-settlement.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - 完整workspace all-feature合并门禁和远端CI尚未运行
  - 独立复审和main交付仍待完成
  - semantic strict snapshot需与其它并行lane统一刷新
---

# Plugin callback reconcile 定向验证证据

## 支持的结论

`failed_withdrawal_blocks_reconcile_and_direct_activation_until_retry`证明旧项首次撤销
失败后新项`init/activate`均未调用；直接激活同样被拒绝，已活跃健康项仍可幂等调用，
旧项成功重试后新项只激活一次。
`failed_deactivate_all_blocks_separate_activation_phase`覆盖分阶段API；
`failed_activation_blocks_later_callbacks_until_cleanup`覆盖部分激活失败后阻断后续项，
并经`unregister`清理恢复。

## 来源与范围

回归由`echo-core/src/plugin/lifecycle.rs`中的Manager真实API驱动，无替身manager或产品
层包装。测试覆盖旧撤销失败、独立阶段调用、直接调用和部分激活失败。

### 执行结果

在基准`0415ba15eb8d348f357fe55df4448897677e6960`的独立Issue #74 worktree上，
`cargo fmt --all -- --check`退出0；
`cargo test -p echo_core plugin::lifecycle::tests --locked` 7/7通过；
`cargo test -p echo_core --locked` 372/372单测通过，doctest 17通过、2忽略；
`cargo clippy -p echo_core --all-targets --locked -- -D warnings`退出0。

## 已知缺口

当前严格semantic verifier检测到并行候选使baseline digest与#134历史`source:`快照
不一致；这不构成Issue #74业务验证通过的替代。完整workspace门禁、独立复审与远端
main交付仍须在集成阶段完成，Finding暂保持open。
