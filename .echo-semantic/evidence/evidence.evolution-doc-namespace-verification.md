---
schema_version: 1
id: evidence.evolution-doc-namespace-verification
kind: evidence
observed_at: source:6190fcbfc03f3cea056d856080ae4b8aa6ba1d2ad9b433823d4aea784d7487ed
source_refs:
  - src/evolution/layer.rs
  - docs/en/25-self-improvement.md
  - docs/zh/25-self-improvement.md
  - docs/en/03-memory.md
  - docs/zh/03-memory.md
  - echo-agent-learning/tests/documentation_contract.rs
supports: [finding.evolution-doc-namespace, behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - 尚未执行完整 workspace 合并门禁和远端 CI
  - 未对任意第三方 Store 执行旧 namespace 数据迁移测试
  - 独立复审和 main 交付尚未完成
---

# Evolution namespace 文档验证

## 支持的结论

### 直接证据

基准 `origin/main@7ba07cbf72d606c4676440d5a3382df8acaa1a8d` 的
双语 25 页均仍含 `typed_memories` 和默认三层承诺，双语 03 页交叉引用
也写热/暖/冷管理。新 `evolution_memory_docs_match_runtime_namespaces`
从公开 `WARM_NAMESPACE`/`COLD_NAMESPACE` 常量生成预期表项，并检查
旧 namespace、默认三层描述、cold 可选性以及通用 Store 示例与
manager 固定 namespace 的边界。修复后执行
`cargo test -p echo-agent-learning --test documentation_contract --locked`：
15 passed、0 failed；`cargo clippy -p echo-agent-learning --test documentation_contract --locked -- -D warnings` 退出码 0；
`cargo fmt --all -- --check` 与 `git diff --check` 均退出码 0。

首次 focused 测试曾因新断言对英文 `Optional` 大小写敏感而失败，
已修正断言并重新运行。该失败不作为旧文档红测证据；旧文档反例由
`git show 7ba07cb:docs/en/25-self-improvement.md` 与对应中文路径的
命中行直接证明。

## 来源与范围

此候选只验证文档与真实公开 namespace 常量保持一致，未验证运行时
读写实现的新行为；后者本轮没有变更。完整合并门禁留在发 PR 或合 main 前。

## 已知缺口

独立复审、最新 main 的全局 source digest 刷新、完整合并门禁与远端 CI
由最终交付阶段处理。目前 strict semantic verifier 退出码 1：
49 条既有跨边界 `source:` 引用和 1 条共享 baseline 摘要失配；
本分支自己的 evidence schema 错误已修复。
