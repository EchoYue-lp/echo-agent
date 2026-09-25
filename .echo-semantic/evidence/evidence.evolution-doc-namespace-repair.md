---
schema_version: 1
id: evidence.evolution-doc-namespace-repair
kind: evidence
observed_at: 07e4270380c40df3f412f99c0aecb6145410cb2a
source_refs:
  - echo-state/src/memory/store.rs
  - src/evolution/layer.rs
  - src/evolution/recall.rs
  - src/evolution/runtime_integration.rs
  - docs/en/25-self-improvement.md
  - docs/zh/25-self-improvement.md
  - docs/en/03-memory.md
  - docs/zh/03-memory.md
  - echo-agent-learning/tests/documentation_contract.rs
supports: [finding.evolution-doc-namespace, behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - 文档同步不迁移旧 namespace 下的消费者数据
  - 不实现独立 cold 存储或改变 MemoryLayerManager 公共 API
---

# Evolution namespace 文档修复

## 支持的结论

### 权威与反例

`MemoryLayerManager` 的 `WARM_NAMESPACE` 为 `["agent", "memories"]`，
`locate`、写入、归档和分层召回均使用该 namespace。暖层降级只将
`MemoryStatus` 更新为 `Archived`，条目仍留在原 namespace；
`COLD_NAMESPACE` 和 `MemoryLayer::Cold` 是保留的公开选项，manager 不读写
独立 cold namespace。修复前双语 25 页代码块和 namespace 表使用
`typed_memories`，总览与 03 页交叉引用承诺默认热/暖/冷三层，文件树还把
warm/cold 描述为 `memory/topics` 与 `memory/archive` 目录。

### 修复

双语页面改用代码导出的 `WARM_NAMESPACE` 常量示例和真实 Store KV 表，
将 `Archived` 说明为暖层状态，并指出独立 cold tier 需要消费者自行实现。
文件布局只列 manager 的 hot 文件、operation journal 与 integration builder
的默认 ChangeLog 路径，不再把产品或技能文件误归入默认分层记忆布局。
底层 `TypedMemoryStore` 示例明确不能代替 manager 的恢复、审计与激活路径。
03 页交叉引用同步为热/暖两层，并将通用 Store 的调用方自定义
namespace 示例与 manager 固定 `WARM_NAMESPACE` 区分；分层记忆隔离
必须同时覆盖 Store 底层路径/分区及 manager 的 hot/journal root 与
ChangeLog 路径。多个 FileStore 句柄指向同一 canonical 路径时共享权威，
仅换句柄或 conversation ID 不构成隔离。

## 来源与范围

这是公开文档和可执行文档契约修复，没有修改持久化格式、运行时代码、
示例程序或架构决策，因此无需新 ADR。旧 namespace 数据兼容/迁移仍需
消费者依据实际历史布局单独评估，本修复不宣称自动导入。

## 已知缺口

整合 `origin/main@9abdf9de` 后的 source digest 与 baseline 已刷新，
独立增量复审及本地完整合并门禁已通过；远端 CI 与 main 交付仍待完成。
