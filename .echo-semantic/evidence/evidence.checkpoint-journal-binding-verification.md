---
schema_version: 1
id: evidence.checkpoint-journal-binding-verification
kind: evidence
observed_at: 29cea08cd3b487d59fef1a769e5d6fe9dbf3e36e
source_refs:
  - echo-state/src/journal/mod.rs
  - echo-state/src/journal/file.rs
  - echo-state/src/journal/segmented.rs
  - src/state/mod.rs
  - tests/acp_agent_adapter.rs
  - docs/en/41-persistence-concepts.md
  - docs/zh/41-persistence-concepts.md
  - docs/adr/0055-checkpoint-journal-identity.md
supports: [behavior.observation-persistence, rule.fact-projection-separation]
limitations:
  - 远端Linux、Windows、dependency与SDK CI等待PR执行
  - validation used macOS local file semantics and did not inject process crash or power-loss faults beyond existing deterministic fault fixtures
---

# Checkpoint 与 Journal generation 绑定验证证据

## 支持的结论

`CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 cargo test -p echo_state journal --locked`执行116项Journal相关测试，116 passed、0 failed、172 filtered out。通过项包含同序号异源checkpoint重建、异源committed receipt在fold前拒绝、同路径File Journal换代、identity-free schema v1拒绝、File mixed frame、Segmented跨segment mixed generation、checkpoint/batch/marker identity tamper、prune/reopen identity保留，以及pruned Journal拒绝异源checkpoint。

`cargo check -p echo_state --locked`、`cargo check -p echo_agent --locked`和`cargo check -p echo_agent --test acp_agent_adapter --features acp --locked`在相同低资源环境下通过，证明`JournalIdentity`、`EventJournal`、`CheckpointStore`、root facade与非默认ACP测试实现的公共契约可编译。

两档定向Clippy均通过：`cargo clippy -p echo_state -p echo_agent --all-targets --locked -- -D warnings`，以及lib/bins的`-D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic -D clippy::unreachable`。`cargo fmt --all`和`cargo fmt --all -- --check`均退出0。

## 来源与范围

命令在`fix/Echoyue/checkpoint-journal-identity`的实现候选`dcd25a8a19c21c3247965e527684d89c96d68ac7`形成前后对同一源码内容执行。独立只读review与后续增量review均pass；高风险semantic strict snapshot和base require-change-evidence也在候选形成前退出0。

集成分支在最终实现后执行`./scripts/verify.sh`，91项SDK生成物、两档workspace Clippy、全部
all-target/all-feature测试与bench、no-default check均通过；17个独立feature全部编译通过。
TypeScript 157项、Python 177项、Java Maven测试与真实Host连接通过。Journal identity public面已按
5622 external、1774 Host/Rust-only、790 language intrinsic、90 internal helper、1448 deferred冻结，
没有新增逐identity语言包装。

## 已知缺口

远端Linux/Windows、dependency policy及SDK CI等待PR执行；本地验证使用macOS文件语义，未执行真实
断电注入。
