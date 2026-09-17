---
schema_version: 1
id: audit.sdk-repository-extraction-rereview
kind: audit
boundary_ref: boundary.workspace-architecture
lens: contract_evidence
freshness: stale
revision: source:bfa5b4590c617d8286f2b80571d5c450622d47c7425d6f1ecb1978bf85743352
finding_refs: [finding.sdk-repository-extraction]
challenges:
  source-ownership:
    revision: source:bfa5b4590c617d8286f2b80571d5c450622d47c7425d6f1ecb1978bf85743352
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

初始候选复审未发现 extraction 范围内新增问题；合流 `0e09324a` 后该结论失效。最终复审必须重新检查 PR #124/#125 行为保留、ADR 0051、SDK Wave 2 payload 握手和当前语义快照。

## 残余风险

独立 SDK PR #1 `6f743d1` 的8项远端CI已全绿，但仍pin初始extraction revision、尚未吸收
Wave 2 Journal identity payload，且semantic baseline无效。Framework候选只能作为下一次
精确pin输入，不能提前合入main。

## 未检查项

未对合流结果运行framework full gate、17-feature matrix、semantic continuity或最终独立复审；SDK full contract、Host E2E、三语言quickstart与远端CI由独立SDK结果负责。
