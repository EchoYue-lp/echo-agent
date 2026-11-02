---
schema_version: 1
id: evidence.stage0-generic-diagnostics
kind: evidence
observed_at: source:66492a65e2a285bf2a13afedea2242dcc834f4a11d80715240c9e8db33aa0880
source_refs:
  - src/plugin/prepared.rs
  - docs/adr/0090-a2a-lsp-retirement-contract.md
  - docs/zh/32-plugin-system.md
supports: []
limitations:
  - 本证据只绑定 F517 candidate 与 Stage0 generic diagnostics；不闭合 A2A/LSP 消费者退役。
  - 本证据不替代正常 Source/final、main CI、native host 或发布验收。
---

## 支持的结论

## 来源与范围

本证据只绑定 F517 `51726454985fd6f064b21a7e9c539e22d494c987` 的 2333 files/283 dirs
snapshot，以及 `src/plugin/prepared.rs` 的唯一 `0f2fa4399c433c02a3664ff8bb3ad34090822c7938fb385cf268464790203471`
增量。应用后 prepared 源 blob 为
`6277087c89a598e08e390077e8a9ef0a6b2fe2157418d58c10624d1aa654139a`。

依赖输入固定为 F517 lock `f76fe10e64ec3faf062ff5bc7a66d6cd82f82cf95e989c824f40fe6f244d234c`
和 duplicate baseline `9ae90ac986f03a16df3213a777ff821e6849f40e7284fa0acc9ae5d89ca02aa3`。
本材料不继承 F340、lock1442、旧 58 项资格或其它 accepted risk。

## 已知缺口

- 未启用 `lsp` 时，既有 prepare/resolver owner 冻结声明来源并产生 generic Warning/Error；真实缺失保持 optional。
- 启用 `lsp` 时保留原冻结路径；健康 Skill、Hook、MCP、Subagent 不受影响。
- 沿用 `PluginPreparationDiagnostic`、PreparedPluginSet identity/generation/cache 和 rollback owner；没有第二 decoder、第二 cache、第二执行器、兼容 wire 或新状态机。
- examples 没有公共 API 迁移；英文插件说明保持历史表述，新增事实只维护中文 ADR/插件说明；官网不在本候选范围。

## 已有结果与未完成门禁

已有 focused 结果：fmt、8 个 no-LSP exact list/body、mcp/lsp boundary、受影响 crate mcp Clippy，及其 raw/run/control/source guard；这些结果只绑定各自 snapshot/执行 epoch。原 2f812 Stage0 source/final 收据仍作为历史事实保留；当前 `project_rules.rs` 跟进后的 Source/final 与 full24 尚待重新验证：

1. 保留历史 Source/final cut：`../pre-main-generic-diagnostics-20261101-r03/stage0-source-final-20261031232808879`（非当前资格）；当前新源必须重新取得 Source/final 与完整 24 步工程门禁。
2. 当前局部 focused 复核为 `7 passed / 0 failed / 442 filtered`、natural exit `0`、warning `0`；这不冒充完整 24 步。新增 fmt/clippy 均自然退出 0：fmt raw `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`，fmt receipt `7bab82f17a2ebdbf1f1839446ebfd09970bb8567ff97bfb1bec460457b612bdb`，fmt check receipt `ebb9208c88b900a386038d25f3cf09369eee653f81ae56859a9e4d269b64dbfa`，echo-core Clippy raw `4d6dedc9580562c866d84d862b90230852b8afbab2712d5e0c508b6814c6a87b`，receipt `ec4d1de60b2efa82cdcbaf6a888c4b417bafadf5f58f8f1f1f97ad53f7e93ecc`。
3. 合入 main 前的 Framework 原仓完整门禁：fmt、workspace/all-targets/all-features tests/Clippy、no-default 与独立 feature checks、精确 cargo-deny 49/107 baseline、14 policy controls、任务分支合 main 后的精确 CI。
4. A2A/LSP 消费者先去依赖与真正 producer/consumer 原子切换仍按 ADR0090、Plan04 独立验收；本 Stage0 不修改 native event 或 producer。

## 反向关联

- 设计与交付顺序：`docs/supreme/specs/eko-gui-product-convergence/plans/plan_04_a2a-lsp-consumer-decoupling.md`、`plans/delivery-map.md#a2a-lsp-consumer-decoupling`。
- 长期边界：`docs/adr/0090-a2a-lsp-retirement-contract.md`。
- 生命周期 Map 保持 F517 基线；本候选不新增 Map 条目，避免把 Stage0 change-evidence 误写入既有高风险闭包。
- 本证据现在与上述正常 Source/final 收据绑定，但仍不是 main CI、native、SDK pin 或发布收据。

## 当前候选 Git 增量（Source/final 绑定输入）

原始 Stage0 记录绑定的是 F517 archive，当时 HEAD/index=F517，唯一 Rust 增量为 `src/plugin/prepared.rs`；该历史事实与旧失败保持不变。当前 candidate 已是 HEAD/index=`2f8123246f51a56b6a247cdf18423df197390c50`，未暂存三路径为 `echo-core/src/project_rules.rs`、本 Evidence 与 ADR；中文插件文档仍沿用已提交内容。本次中文文档/evidence 增量仅属于当前 candidate 的未暂存交接输入：

| 路径 | SHA-256 | 用途 |
| --- | --- | --- |
| `docs/adr/0090-a2a-lsp-retirement-contract.md` | `0c604d049e52592ff8d259c9b782d4ab72a5b7ffc65fda6ea1e4bb1d13f355cb` | Stage0 适用边界与非目标 |
| `docs/zh/32-plugin-system.md` | `b06c48c14830cce5362fe9b0d6ea607accff69a216939bc4c63de11c161dc319` | 中文插件系统行为说明 |
| `echo-core/src/project_rules.rs` | `36eb36c491df8acb57aedab4190589426708386baf1cf2bf9c6ad424c9ae393b` | private/test-only discovery fixture 修复 |
| `.echo-semantic/evidence/evidence.stage0-generic-diagnostics.md` | 由本次 Source/final 收据的 task evidence 绑定 | 当前候选 change-evidence |

## focused 源码跟进

当前 candidate 另有一处局部、框架 private/test-only 修复：`echo-core/src/project_rules.rs` 将既有扫描主体抽为 private `resolve_at_root`，使 nonGit 测试显式验证无根解析分支；public Git discovery、生产行为和公开 API 均保持不变。该源的 focused gate 使用同一 F517 warm target、冻结工具链和原测试合同，实际结果为 `7 passed / 0 failed / 442 filtered`、natural exit `0`、warning `0`；程序/FP/.d 与原 RED 票据分别由外部 handoff 绑定。Examples 与官网无适用变更，后续 Source/final 必须绑定当前 candidate source。

这些文档路径不属于原 `cd8735…` 基础 manifest 的 Rust/运行输入；Source/final 必须把它们作为本次 Git-backed candidate 的变更证据逐项绑定。Map 保持 F517 基线；英文插件文档与 examples 保持历史/不适用结论，不创建形式英文副本或公共 API 迁移。
