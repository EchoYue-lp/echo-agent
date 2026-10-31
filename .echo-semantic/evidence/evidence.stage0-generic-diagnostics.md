---
schema_version: 1
id: evidence.stage0-generic-diagnostics
kind: evidence
observed_at: source:bdd09d7e643442fbf798408db20f265591a7c154ca540ce98cee624b1748b644
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

已有 focused 结果：fmt、8 个 no-LSP exact list/body、mcp/lsp boundary、受影响 crate mcp Clippy，及其 raw/run/control/source guard；这些结果只绑定各自 snapshot/执行 epoch。当前候选已补齐 task-scoped Source/final：

1. 保留历史 Source/final cut：`../pre-main-generic-diagnostics-20261101-r03/stage0-source-final-20261031232808879`（非当前资格）；当前资格由候选 `verification.json` 的 result/contentDigest 与外部 handoff 索引绑定。
2. 文档/示例适用性与 staged artifact contract 的独立交接记录仍待补齐；final `verification.json` 继续记录 task contentDigest，`globalClosure=not_checked`。
3. 合入 main 前的 Framework 原仓完整门禁：fmt、workspace/all-targets/all-features tests/Clippy、no-default 与独立 feature checks、精确 cargo-deny 49/107 baseline、14 policy controls、任务分支合 main 后的精确 CI。
4. A2A/LSP 消费者先去依赖与真正 producer/consumer 原子切换仍按 ADR0090、Plan04 独立验收；本 Stage0 不修改 native event 或 producer。

## 反向关联

- 设计与交付顺序：`docs/supreme/specs/eko-gui-product-convergence/plans/plan_04_a2a-lsp-consumer-decoupling.md`、`plans/delivery-map.md#a2a-lsp-consumer-decoupling`。
- 长期边界：`docs/adr/0090-a2a-lsp-retirement-contract.md`。
- 生命周期 Map 保持 F517 基线；本候选不新增 Map 条目，避免把 Stage0 change-evidence 误写入既有高风险闭包。
- 本证据现在与上述正常 Source/final 收据绑定，但仍不是 main CI、native、SDK pin 或发布收据。

## 当前候选 Git 增量（Source/final 绑定输入）

Root 已将 F517 archive 绑定为 Git-backed candidate；当前工作树保持 HEAD=F517、index=F517，唯一 Rust 修改仍是 `src/plugin/prepared.rs`。本次中文文档/evidence 增量仅属于该候选的未暂存交接输入：

| 路径 | SHA-256 | 用途 |
| --- | --- | --- |
| `docs/adr/0090-a2a-lsp-retirement-contract.md` | `aac0042748f0ca29e45a9424bdb6774beb6b6cf1b854891189cfcd174c946055` | Stage0 适用边界与非目标 |
| `docs/zh/32-plugin-system.md` | `b06c48c14830cce5362fe9b0d6ea607accff69a216939bc4c63de11c161dc319` | 中文插件系统行为说明 |
| `.echo-semantic/evidence/evidence.stage0-generic-diagnostics.md` | 由本次 Source/final 收据的 task evidence 绑定 | 当前候选 change-evidence |

这些文档路径不属于原 `cd8735…` 基础 manifest 的 Rust/运行输入；Source/final 必须把它们作为本次 Git-backed candidate 的变更证据逐项绑定。Map 保持 F517 基线；英文插件文档与 examples 保持历史/不适用结论，不创建形式英文副本或公共 API 迁移。
