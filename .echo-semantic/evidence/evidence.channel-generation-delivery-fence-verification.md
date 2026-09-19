---
schema_version: 1
id: evidence.channel-generation-delivery-fence-verification
kind: evidence
observed_at: source:3a8aba9cee4bdf94c17e2039bb9bc22ebcf6fb112bfe26ce5686002569409849
source_refs:
  - echo-integration/src/channels/session.rs
  - echo-integration/src/channels/types.rs
  - echo-integration/src/channels/channels/mod.rs
  - echo-integration/src/channels/channels/qq/channel.rs
  - echo-integration/src/channels/channels/feishu/channel.rs
  - echo-agent-learning/examples/demo38_im_channels.rs
  - docs/adr/0057-channel-generation-delivery-fence.md
supports: [finding.channel-reset-stale-generation-delivery, behavior.protocol-projection, rule.protocol-role-separation]
limitations:
  - 最新main集成、17项feature矩阵和完整本地合并门禁已完成；远端CI仍待PR交付阶段核实
  - 测试使用可控本地delivery receipt，不对QQ或飞书真实远端的时序作等价声明
---

# Channel generation delivery fence 验证

## 支持的结论

三个生产反例均先取得行为红灯，再完成修复：

- `reset_waits_for_admitted_delivery_and_fences_later_old_chunks`：原实现0/1，错误为
  `reset acknowledged before admitted old delivery settled`；修复后1/1。
- `reset_cancels_blocked_stream_setup_and_settles_cleanup`：原实现0/1，错误为
  `retired stream setup remained blocked`；setup cancellation修复后1/1。
- `framework_reset_waits_for_delivery_admitted_before_application_rotation`：原实现0/1，
  错误为`reset ignored delivery admitted before application rotation`；共享generation计数后1/1。

最新已审行为快照执行：

- `cargo test -p echo_integration --features channels --locked`：128 unit + 2 doctests，0 failed。
- QQ与飞书direct send retired-generation回归：2/2，均先返回typed stale而非not-started/network。
- `cargo clippy -p echo_integration --all-targets --features channels --locked -- -D warnings`：exit 0。
- `cargo check -p echo-agent-learning --example demo38_im_channels --features channels --locked`：exit 0。
- 17项独立feature check：acp、a2a、mcp、lsp、sqlite、telemetry、topology、subagent、web、
  media、data、statistics、channels、git、database、rag、chart，17次Finished、exit 0。
- 完整合并门禁的原始六条命令全部exit 0：`cargo fmt --all`、fmt check、workspace
  all-target/all-feature Clippy、lib/bins panic-policy Clippy、workspace all-target/all-feature tests、
  workspace lib no-default check。全量测试82条result汇总：2819 passed、0 failed、3 ignored。

## 来源与范围

日志保存在该worktree Git状态目录`supreme/logs/issue41-*.log`。独立reviewer审查完整diff、
直接调用方和测试，先发现setup cancellation、application rotate permit遗失及public mutable fence，
三项修复后最终结论pass；随后要求的QQ/飞书direct-send测试2/2通过。

## 已知缺口

本Evidence在任务检查点commit `c6c9b04d`之后仅增加语义材料。远端main仍为
`ab3ed7d2`，无需额外合并。矩阵日志为`issue41-feature-matrix-1789666763874.log`；完整门禁按
磁盘边界逐项运行，并在高成本步骤间只清理本会话`CARGO_TARGET_DIR`：
`issue41-final-{fmt,fmt-check,clippy-all-targets,clippy-panic-policy,tests-all-features,no-default}-*.log`。
资源配置仅降低并发、incremental与debug symbols，不关闭测试、lint、feature或debug assertions。
PR远端CI通过前不关闭GitHub Issue。
