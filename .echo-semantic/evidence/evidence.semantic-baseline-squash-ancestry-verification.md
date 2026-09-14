---
schema_version: 1
id: evidence.semantic-baseline-squash-ancestry-verification
kind: evidence
observed_at: source:8ff7eb397767728069b01b9098b224a6840a8adb663717e5c4fd7a584eb4063e
source_refs:
  - .echo-semantic/baseline.md
  - .github/workflows/rust-ci.yml
  - AGENTS.md
  - docs/adr/0041-semantic-governance-continuity.md
  - echo-agent-learning/tests/semantic_baseline_contract.rs
supports: [behavior.workspace-composition, rule.framework-layer-ownership]
limitations:
  - 候选验证不替代follow-up PR的远端CI和真实merged main复验
  - Issue #118及原22个resolved Finding Issue在follow-up进入main前保持OPEN
---

# Semantic baseline squash ancestry验证证据

## 支持的结论

PR #117合并后的原始main稳定复现strict失败：baseline `base_revision=7ba1f122...`不是`HEAD=d492c676...`的祖先。Git对照确认`7ba1f122 -> d492c676`返回非祖先，而`b21aba01 -> d492c676`和`d492c676 -> d492c676`均为祖先。

将baseline YAML和正文单点改绑`d492c676d1bf0744452d96a6960124546ed3fff9`后，同一strict snapshot exit 0。新增learning contract但尚未修改workflow时，3个测试稳定为2 passed/1 failed，失败精确是缺少`ECHO_SEMANTIC_TARGET_REVISION`；CI传入PR base或push before SHA并fetch完整历史后，同一命令3/3通过，focused Clippy `-D warnings`通过。

持久合同用`serde_yaml_ng`解析真实baseline；本地默认解析`origin/main`/`main`，CI显式提供target revision。临时Git反例证明feature-only SHA是feature HEAD祖先，却不是target main或squash result祖先；target main继续是squash result祖先。完整`./scripts/verify.sh`于2026-09-14T05:45:32Z至05:54:50Z运行，exit 0；以`d492c676`为base的task-scoped change-evidence、strict和94/94 Issue对账均通过。

格式稳定后的`content_digest=8ff7eb397767728069b01b9098b224a6840a8adb663717e5c4fd7a584eb4063e`；inventory/behavior closure、Capability Map集合和runtime/SDK源码未变化。包含全部source与语义更新的临时候选tree`31fccf619a84bfdf91794d7161c526a0f72470d6`对`b21aba01b34e74c93d783a89db895282ba831c3c`运行continuity，428个义务为408 preserved、4 replaced、16 retired，0 unresolved，`passed=true`且errors为空。

## 来源与范围

验证使用原strict失败、Git ancestor矩阵、focused red/green contract、focused Clippy、完整仓库门禁、task-scoped change-evidence、候选Git tree continuity、Issue对账和staged diff。Source改动只涉及learning contract、Rust CI和同步AGENTS；没有新增依赖或修改runtime/API/SDK。

## 已知缺口

独立rereview为PASS，Critical、Important、Minor均为0。Follow-up提交/PR/远端CI/merge及merged main复验尚未执行，Issue生命周期在这些步骤完成前不关闭。
