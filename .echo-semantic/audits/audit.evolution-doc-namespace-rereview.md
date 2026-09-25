---
schema_version: 1
id: audit.evolution-doc-namespace-rereview
kind: audit
boundary_ref: boundary.eval-evolution
lens: data_durability
freshness: examined
revision: source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74
finding_refs: [finding.evolution-doc-namespace]
challenges:
  warm-and-archived-authority:
    revision: source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74
    source_refs: [src/evolution/layer.rs, src/evolution/recall.rs, docs/en/25-self-improvement.md, docs/zh/25-self-improvement.md]
    evidence_refs: [evidence.evolution-doc-namespace-repair, evidence.evolution-doc-namespace-verification]
  store-and-file-isolation:
    revision: source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74
    source_refs: [echo-state/src/memory/store.rs, src/evolution/layer.rs, src/evolution/runtime_integration.rs, docs/en/03-memory.md, docs/zh/03-memory.md]
    evidence_refs: [evidence.evolution-doc-namespace-repair, evidence.evolution-doc-namespace-verification]
  executable-doc-contract-and-snapshot:
    revision: source:b9df4130cf07d2561e8e61ec24d127607ce21112eca995952c64c1290ede3b74
    source_refs: [echo-agent-learning/tests/documentation_contract.rs, .echo-semantic/baseline.md]
    evidence_refs: [evidence.evolution-doc-namespace-verification]
---

# Evolution namespace 独立增量复审

## 审查范围

独立复审以 `a81e1752e5262e6109c3afab1ae3a411de0ed0f1` 为整合代码锚点，
检查 #53 双语文档、公开 namespace 常量、manager/Store/ChangeLog 隔离与
`documentation_contract`。另外核对了在此快照上的 45 个 `.echo-semantic`
文件增量：除本 Finding 的验证说明外，均为当前源码摘要字段的机械刷新。

## 已检查故障假设

- 文档仍将 `typed_memories` 或独立 cold namespace 说成默认读写路径。
- 不同 `FileStore` 句柄、相同 canonical backing path 或不同
  `conversation_id` 被误当作分层记忆隔离。
- Store 分区已隔离，但热层 `MEMORY.md`、operation journal 或
  ChangeLog 路径仍共享，造成跨 Agent 事实和恢复状态混用。
- 文档契约只检查正文存在常量，无法发现 namespace 表再次漂移；
  整合后的语义摘要与 baseline 不一致。

## 实际实现路径与证据

`MemoryLayerManager` 默认使用 `WARM_NAMESPACE = ["agent", "memories"]`；
`Archived` 是 warm 内状态，`COLD_NAMESPACE` 仅为消费者自建独立层保留。
双语 25 页的示例和表格、双语 03 页的隔离说明与该路径一致。
`FileStore::new` 对同 canonical 路径共享权威，隔离必须同时覆盖 Store
backing path/partition、manager hot/journal root 及 ChangeLog 路径。
文档契约从公开常量生成表项并检查这些边界；合入最新 main 后
documentation contract 15/15 通过，strict-snapshot/change-evidence
及 `git diff --check` 均退出码 0。

## 问题记录

独立增量复审对修订后的 #53 实现返回 PASS，没有剩余 Critical 或
Important 问题。先前“不同 Store instance 即隔离”的 Important 文档错误
已在 `ec9fb31e` 修正。语义台账此前缺少 rereview audit 和更新后的
摘要状态，本记录补齐该闭合链。此后同一整合快照上的
`./scripts/verify.sh` 退出码 0，Finding 可置 resolved；GitHub Issue
仍待远端交付。

## 残余风险

本修复不自动迁移消费者旧 `typed_memories` 数据，也不提供独立 cold
实现。完整 workspace 合并门禁已通过，远端 CI 与 main 交付尚未完成；
这些是关闭 Issue #53 前的独立验收步骤。

## 未检查项

未执行第三方 Store 的真实旧数据迁移、各产品的隔离配置或远端 CI。
本次只有文档与文档契约变更，不触发独立 feature 矩阵。
