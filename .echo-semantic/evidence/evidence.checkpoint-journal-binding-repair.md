---
schema_version: 1
id: evidence.checkpoint-journal-binding-repair
kind: evidence
observed_at: source:f53372ae92a88027ba3de7c1b6551d80c9cfa2e9f06668076cb206bb80eb0fbd
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
  - full workspace all-feature integration gate and remote CI remain pending for the final delivery branch
  - independent source and incremental test reviews passed; a revision-bound rereview Audit is recorded only after the implementation candidate commit exists
  - schema version 1 Journal and checkpoint files are rejected rather than migrated during the current pre-stable development phase
---

# Checkpoint 与 Journal generation 绑定修复证据

## 支持的结论

`EventJournal`现在显式拥有`JournalIdentity`。内存、单文件和分段 Journal 都把同一 generation identity 传入 batch frame；文件 batch digest 覆盖 identity，分段 retained-floor marker 在删除 sequence 1 所在 segment 后继续保存并校验 identity。

`CheckpointStore::save`接收 Journal identity，`CheckpointFrame`在 load 后返回该 identity，文件 checkpoint digest 同时覆盖 identity、sequence 与 reducer state。append receipt也携带提交 Journal identity，`apply_committed`在fold前拒绝异源receipt。`CheckpointedReducer`在任何 sequence 接受前比较来源：完整 Journal 遇到异源 checkpoint 时从 sequence 0 重建并修复，prefix 已裁剪时因缺失事实不可恢复而失败关闭。

## 来源与范围

修复扩展现有`EventJournal`、`CheckpointStore`、`CheckpointedReducer`和内建 file/memory/segmented backend，不新增 store、reducer 或 Workflow checkpoint 路径。ADR 0055记录 LangGraph、Flink、Axon 的来源/所有者 identity 共性、框架分层、schema v2 与 pre-stable 兼容策略。

## 已知缺口

源码中的反例覆盖同序号异源内存 Journal、异源committed receipt、同路径文件 Journal 换代、identity-free schema v1拒绝、mixed File frame与跨segment generation、identity digest tamper、prune/reopen identity 保留，以及 pruned Journal 拒绝异源 checkpoint。独立源码审查与增量测试构造复核均已pass，最终116项Journal focused tests、direct checks、两档Clippy与fmt也全部通过；本 Evidence 仍只描述修复候选，Finding保留到integration final gate。
