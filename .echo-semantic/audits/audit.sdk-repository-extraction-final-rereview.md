---
schema_version: 1
id: audit.sdk-repository-extraction-final-rereview
kind: audit
boundary_ref: boundary.workspace-architecture
lens: contract_evidence
freshness: examined
revision: 27c7701e1eb116db1076da7f84bb68898544a44c
finding_refs: [finding.sdk-repository-extraction]
challenges:
  framework-only-boundary:
    revision: 27c7701e1eb116db1076da7f84bb68898544a44c
    source_refs: [Cargo.toml, README.md, README.zh.md, .github/workflows/rust-ci.yml]
    evidence_refs: [evidence.sdk-repository-extraction-final-verification]
  external-sdk-owner:
    revision: 27c7701e1eb116db1076da7f84bb68898544a44c
    source_refs: [docs/adr/0051-extract-sdk-repository.md, README.md]
    evidence_refs: [evidence.sdk-repository-extraction-final-verification]
  retained-runtime-authority:
    revision: 27c7701e1eb116db1076da7f84bb68898544a44c
    source_refs: [tests/acp_agent_adapter.rs, echo-agent-learning/tests/documentation_contract.rs]
    evidence_refs: [evidence.sdk-repository-extraction-equivalence, evidence.sdk-repository-extraction-final-verification]
---

# SDK repository extraction final re-review

## 审查范围

复审 framework-only workspace、SDK-owned 路径删除、保留的 ACP/runtime authority、最终
framework revision、独立 SDK owner、Wave 2 inventory payload、双仓 merge ancestry 和
Issue #122 交付证据。

## 已检查故障假设

- framework 删除 SDK 路径后没有独立 owner 或精确 dependency provenance；
- extraction 误删通用 ACP/runtime 或 learning consumer；
- SDK squash/merge 丢失 source-continuity lineage；
- PR #124 Journal identity payload 未进入 SDK contract；
- semantic evidence 仍停在旧 candidate revision。

## 实际实现路径与证据

Framework main `27c7701e` 保留 framework runtime/ACP 入口并完成 SDK-owned 路径迁移。SDK main
`146f69a9` 以 merge commit 保留 `c8941c6` 及其双父 ancestry，Host/lockfile 精确 pin
framework main，generator 输出 9,724 inventory，SDK 8/8 CI 与本地完整门禁均通过。

## 结论

上述故障假设均已由当前源码、Git ancestry、独立 SDK contract/Host/language gate 和
semantic strict/change-evidence 检查；本 Audit 标记为 examined，Finding 的修复、验证和复审闭合。

## 问题记录

最终复审未发现 Issue #122 范围内的新反例；历史 candidate Evidence 继续保留作为迁移过程记录，
最终闭合以本 Audit 和 final verification Evidence 为准。

## 残余风险

SDK 仍为源码优先交付，未发布预编译制品；下游自行选择 framework revision 时仍需使用
SDK compatibility 声明和 lockfile provenance。

## 未检查项

未检查预编译制品、registry 发布、未来 framework revision 或仓库外私有 consumer。
