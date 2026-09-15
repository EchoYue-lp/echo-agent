---
schema_version: 1
id: audit.semantic-baseline-squash-ancestry-rereview
kind: audit
boundary_ref: boundary.workspace-architecture
lens: contract_evidence
freshness: examined
revision: source:9292679f8fdc376a08aaf80755281bc316908ec9915dfb190014676318de8abb
finding_refs: [finding.semantic-baseline-squash-ancestry]
challenges:
  target-main-not-feature-head:
    revision: source:9292679f8fdc376a08aaf80755281bc316908ec9915dfb190014676318de8abb
    source_refs: [.echo-semantic/baseline.md, echo-agent-learning/tests/semantic_baseline_contract.rs]
    evidence_refs: [evidence.semantic-baseline-squash-ancestry-repair, evidence.semantic-baseline-squash-ancestry-verification]
  squash-removes-feature-ancestry:
    revision: source:9292679f8fdc376a08aaf80755281bc316908ec9915dfb190014676318de8abb
    source_refs: [echo-agent-learning/tests/semantic_baseline_contract.rs, AGENTS.md]
    evidence_refs: [evidence.semantic-baseline-squash-ancestry-verification]
  ci-supplies-recoverable-target:
    revision: source:9292679f8fdc376a08aaf80755281bc316908ec9915dfb190014676318de8abb
    source_refs: [.github/workflows/rust-ci.yml, echo-agent-learning/tests/semantic_baseline_contract.rs]
    evidence_refs: [evidence.semantic-baseline-squash-ancestry-verification]
  source-and-runtime-scope:
    revision: source:9292679f8fdc376a08aaf80755281bc316908ec9915dfb190014676318de8abb
    source_refs: [Cargo.toml, contracts/sdk/parity-manifest.json, .github/workflows/rust-ci.yml]
    evidence_refs: [evidence.semantic-baseline-squash-ancestry-repair, evidence.semantic-governance-b21-continuity]
---

# Semantic baseline squash ancestry独立复审

## 审查范围

复审PR #117 squash后的strict失败、baseline ancestor选择、target-main合同、临时Git反例、GitHub PR/push revision输入、完整历史获取、source snapshot刷新、Finding/Evidence引用和既有continuity义务。

## 已检查故障假设

检查一次性改绑是否只能修当前实例；选择`b21aba01`是否会降低谱系精度；feature-only SHA是否仍能在PR HEAD上假绿；squash反例是否使用真实Git ancestry；PR base和push before是否可恢复；浅克隆是否让merge-base误判；新测试是否实际进入learning CI；历史Evidence是否被错误改写为当前证据。

## 实际实现路径与证据

Baseline绑定当前canonical main`d492c676`。Learning contract结构化解析baseline，本地解析`origin/main`/`main`，CI显式注入PR base或push before。临时Git仓库证明feature-only SHA是feature HEAD祖先，但不是target main或squash result祖先；target main保持为squash result祖先。Linux learning job checkout完整历史并运行该test target。

测试在workflow未提供target revision时稳定2 passed/1 failed，补齐CI后3/3通过。Focused Clippy、完整`verify.sh`、strict、change-evidence、94/94 Issue对账和continuity均通过。Source digest刷新只来自test/workflow/AGENTS，runtime/API/SDK未修改。独立复审结论为PASS，Critical、Important、Minor均为0。

## 问题记录

首轮review确认`d492c676`是正确当前ancestor，但阻断只做一次性rebind的方案。Plan 15随后扩展为持久target-main门禁与squash反例；复审确认原blocker已关闭，没有新Finding。

## 残余风险

Follow-up PR尚未进入远端main，Issue #118和原22个resolved Finding Issue继续保持OPEN。71个其它open Finding不受本修复影响。

## 未检查项

未修改或发布Echo Semantic插件本身；本修复是echo-agent仓库级pre-merge合同。远端CI、follow-up squash结果和merged main push事件仍需实际验收。
