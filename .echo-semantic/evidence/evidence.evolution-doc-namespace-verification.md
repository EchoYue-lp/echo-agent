---
schema_version: 1
id: evidence.evolution-doc-namespace-verification
kind: evidence
observed_at: source:5e9a01f48cdf8338bbf8ccfdf225290e1c3f3db90fbea6c30cf1219b92cd1f48
source_refs:
  - echo-state/src/memory/store.rs
  - src/evolution/layer.rs
  - docs/en/25-self-improvement.md
  - docs/zh/25-self-improvement.md
  - docs/en/03-memory.md
  - docs/zh/03-memory.md
  - echo-agent-learning/tests/documentation_contract.rs
supports: [finding.evolution-doc-namespace, behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - 未对任意第三方 Store 执行旧 namespace 数据迁移测试
  - 远端 CI、main 交付及 GitHub Issue 关闭尚未完成
---

# Evolution namespace 文档验证

## 支持的结论

### 直接证据

基准 `origin/main@7ba07cbf72d606c4676440d5a3382df8acaa1a8d` 的
双语 25 页均仍含 `typed_memories` 和默认三层承诺，双语 03 页交叉引用
也写热/暖/冷管理。新 `evolution_memory_docs_match_runtime_namespaces`
从公开 `WARM_NAMESPACE`/`COLD_NAMESPACE` 常量生成预期表项，并检查
旧 namespace、默认三层描述、cold 可选性以及通用 Store 示例与
manager 固定 namespace 的边界。独立复审指出最初文档把不同 Store
句柄误作独立 authority；追加的 contract 还检查同路径 FileStore、
hot/journal root、ChangeLog 和 conversation ID 的隔离说明。修复后执行
`cargo test -p echo-agent-learning --test documentation_contract --locked`：
15 passed、0 failed；`cargo clippy -p echo-agent-learning --test documentation_contract --locked -- -D warnings` 退出码 0；
`cargo fmt --all -- --check` 与 `git diff --check` 均退出码 0。

首次 focused 测试曾因新断言对英文 `Optional` 大小写敏感而失败，
已修正断言并重新运行。该失败不作为旧文档红测证据；旧文档反例由
`git show 7ba07cb:docs/en/25-self-improvement.md` 与对应中文路径的
命中行直接证明。

## 来源与范围

此候选只验证文档与真实公开 namespace 常量保持一致，未验证运行时
读写实现的新行为；后者本轮没有变更。合入 `origin/main@9abdf9de`
后，documentation contract 再次通过 15/15；所有当前快照字段与
baseline 刷新到整合源码摘要 `source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74`，
strict-snapshot/change-evidence 校验退出码 0。

在同一 `a81e1752e5262e6109c3afab1ae3a411de0ed0f1` 代码及 #53
当前差异上执行 `./scripts/verify.sh`，2026-09-25 08:23:10–08:34:56 UTC
退出码 0。日志位于
`.git/worktrees/evolution-doc-namespace/supreme/logs/issue53-full-gate-1790324590165.log`，
375260 bytes、未截断；
格式检查、两组 Clippy、workspace all-target/all-feature 测试及
no-default-features lib check 均完成。86 条 `test result` 汇总全部
0 failed，包含既有 3 条 ignored，并非声称所有测试均实际执行。

## 已知缺口

修复 Store 句柄隔离说明后的独立增量复审与本地完整合并门禁已通过；
远端 CI、main 交付和 Issue 关闭仍待完成。历史叙述中的旧 source 摘要保留为历史证据，
不能将过去的日志视为当前整合快照的全量验证。
