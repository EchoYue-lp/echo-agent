---
schema_version: 1
id: evidence.semantic-baseline-squash-ancestry-repair
kind: evidence
observed_at: 0878a676c9128619fdfb2faa15868a1388689822
source_refs:
  - .echo-semantic/baseline.md
  - .github/workflows/rust-ci.yml
  - AGENTS.md
  - docs/adr/0041-semantic-governance-continuity.md
  - echo-agent-learning/tests/semantic_baseline_contract.rs
supports: [behavior.workspace-composition, rule.framework-layer-ownership]
limitations:
  - 只修复PR #117 squash后的baseline ancestry及其pre-merge防回归合同，不修改runtime、SDK或其它Finding状态
  - 远端follow-up PR、合并main复验和Issue关闭尚未执行
---

# Semantic baseline squash ancestry修复证据

## 支持的结论

Repository baseline的`base_revision`和正文现共同绑定PR #117的squash merge结果`d492c676d1bf0744452d96a6960124546ed3fff9`。该revision是当前main自身及后续follow-up commit的祖先，且包含本轮全workspace治理的完整非语义源码结果。

该历史修复快照的`content_digest`因新增learning contract、CI输入和AGENTS职责而刷新为`8ff7eb397767728069b01b9098b224a6840a8adb663717e5c4fd7a584eb4063e`；当时inventory/behavior model closure、11张Capability Map、93个既有Finding和全部runtime/SDK文件均未改变。修复保留strict的ancestor门禁，并增加target-main ancestry合同，没有用放宽validator掩盖squash历史变化。

## 来源与范围

失败证据来自合并后main上的strict snapshot和三组`git merge-base --is-ancestor`对照。持久修复复用learning test、结构化YAML解析和真实CI target SHA，不新增依赖或第二semantic verifier。

## 已知缺口

候选仍需strict、change-evidence、continuity、94/94 Issue对账和独立rereview。Issue #118只在follow-up修复进入远端main后关闭。
