---
schema_version: 1
id: audit.sdk-repository-extraction-rereview
kind: audit
boundary_ref: boundary.workspace-architecture
lens: contract_evidence
freshness: examined
revision: source:298b7209a1d4a5151d191db785daa2e394ab43f75a3bc321d8275f6cdf56c757
finding_refs: [finding.sdk-repository-extraction]
challenges:
  source-ownership:
    revision: source:298b7209a1d4a5151d191db785daa2e394ab43f75a3bc321d8275f6cdf56c757
    source_refs: [Cargo.toml, README.md, README.zh.md, .github/workflows/rust-ci.yml]
    evidence_refs: [evidence.sdk-repository-extraction-equivalence, evidence.sdk-repository-extraction-verification]
    finding_refs: [finding.sdk-repository-extraction]
    result: SDK-owned paths are absent from the framework workspace and the retained ACP/runtime boundary remains present.
---

# SDK repository extraction re-review

## 审查范围

审查删除路径、保留的 ACP/runtime 入口、Cargo workspace、framework-only 文档和 CI。

## 已检查故障假设

检查漏删 SDK ownership、误删 ACP adapter、learning consumer 失效、README topology drift 和 SDK job 残留。

## 实际实现路径与证据

`Cargo metadata` 只列 9 个 framework/learning package；focused documentation contracts、rustfmt 和 shell checks 为零退出。

## 问题记录

当前未发现 extraction 范围内新增问题；SDK 后续合同和独立 Host 仍保留为下一阶段。

## 残余风险

独立 SDK 尚未切换到 framework extraction revision，且 SDK semantic baseline 与 language gates 需要后续集中重建。

## 未检查项

未在本审查中运行 SDK full contract、Host E2E、三语言 quickstart、远端 CI 或 release。
