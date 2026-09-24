---
schema_version: 1
id: audit.checkpoint-plan-authority-rereview
kind: audit
boundary_ref: boundary.context-memory
lens: state_authority
freshness: examined
revision: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
finding_refs: [finding.checkpoint-current-plan-orphan-authority]
challenges:
  legacy-store-compatibility:
    revision: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
    source_refs: [src/state/mod.rs, src/state/file.rs, src/state/sqlite.rs, src/agent/react/tests.rs]
    evidence_refs: [evidence.checkpoint-plan-authority-repair, evidence.checkpoint-plan-authority-verification]
  react-authority-retirement:
    revision: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
    source_refs: [src/agent/react/mod.rs, src/agent/react/run/context.rs, src/agent/snapshot.rs, src/agent/react/run/stream_channel.rs]
    evidence_refs: [evidence.checkpoint-plan-authority-repair, evidence.checkpoint-plan-authority-verification]
  public-contract-alignment:
    revision: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
    source_refs: [README.md, README.zh.md, CHANGELOG.md, src/memory.rs, docs/adr/0008-canonical-runtime-task-authority.md, echo-agent-learning/tests/documentation_contract.rs]
    evidence_refs: [evidence.checkpoint-plan-authority-repair, evidence.checkpoint-plan-authority-verification]
---

# Checkpoint plan authority 独立复审

## 审查范围

独立 reviewer 在 `main@be8cbbc6` 的 Issue #42 worktree 上读取完整 framework diff、
Plan 04、总设计绑定章节、ADR 0008、ReAct hydrate/reset/checkpoint 生产路径、
File/SQLite backend、focused tests、双语文档、README、Changelog、examples 与语义证据。
SDK、CLI、website、A2A 和 #76 按 framework-only 边界排除。

## 已检查故障假设

检查旧非空值在重启或 A -> B -> A 切换后覆盖 canonical Task graph、新 safe point
继续传播 stale plan、取消 hydration 发布半状态、File/SQLite 丢失 raw round-trip、
CAS 或 transcript proof 因字段退役被弱化，以及公共说明继续宣称 plan recovery。

## 实际实现路径与证据

`TaskRevisionService` 继续独占 revisioned Task graph。ReAct 私有 `plan_state` 及其
reset/restore/snapshot 路径已删除，新 checkpoint 写 `current_plan: None`。
File/SQLite 仍按原始 `AgentCheckpoint` 字段读写并参与 CAS/proof 比较。重启、
legacy 非空值、managed transcript settlement、stale identity 与取消路径均有回归。
公开文档和 rustdoc 将字段限定为 legacy Store round-trip，并有 executable
documentation contract 防止旧表述回归。

## 问题记录

首轮复审发现五个公共说明入口仍宣称 ReAct plan recovery，严重程度 Important。
修复并增加 documentation contract 后，增量复审 PASS；Critical、Important、Minor
均为 0，行动项为 0。结论绑定完整 diff
`f441e84bd147e5ca66b3ec9bc911d370a1908976827805018778c5507e3d6860`
与上列 semantic source。

## 残余风险

PR CI、远端 main 交付和 post-merge strict verification 尚未发生；这些是外部 Issue
生命周期门禁，不构成当前实现候选的代码 finding。

## 未检查项

未验证 SDK、CLI、website 或 A2A consumer；它们由本轮 framework-only 范围明确排除。
