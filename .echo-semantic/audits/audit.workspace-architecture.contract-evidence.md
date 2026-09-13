---
schema_version: 1
id: audit.workspace-architecture.contract-evidence
kind: audit
boundary_ref: boundary.workspace-architecture
lens: contract_evidence
freshness: examined
revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
finding_refs: [finding.workspace-topology-doc-drift, finding.public-feature-table-drift, finding.readme-example-target-drift]
challenges:
  workspace-topology-docs:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [Cargo.toml, README.md, README.zh.md]
    evidence_refs: [evidence.workspace-structure]
  public-feature-table:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [Cargo.toml, README.md, README.zh.md]
    evidence_refs: [evidence.workspace-structure]
  readme-cargo-targets:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [README.md, README.zh.md, echo-agent-learning/tests/example_contracts/demo34_workflow_stream.rs, echo-agent-learning/tests/example_contracts.rs, echo-agent-learning/tests/documentation_contract.rs]
    evidence_refs: [evidence.workspace-structure]
---

# Workspace、Feature 与 README 合同证据审计

## 审查范围

审查11-package manifests、root双语README拓扑/feature/命令、learning example-contract targets与CI/documentation checks。

## 已检查故障假设

验证公开crate数量与拓扑是否匹配Cargo、feature表是否引用不存在feature、README命令是否对应真实Cargo target，以及测试存在是否被误作已运行证据。

## 实际实现路径与证据

Root+10 members构成11 package；双语README遗漏echo-sdk-protocol/host并声称8+1。Cargo没有tasks feature且明确Task API属于core，README feature表却列出tasks并在后文给相反说明。demo34是tests/example_contracts模块而非Cargo example target，README命令不可用。现有documentation_contract只检查learning包内部链接/名称，CI运行它也无法发现root README命令漂移。

## 问题记录

新增workspace topology、public feature table和README example target三个纯文档/合同Finding；behavior.workspace-composition在补修与合同测试前降为needs_review。

## 残余风险

当前只审查已发现的三类漂移，未证明所有README代码块、链接或feature组合正确。

## 未检查项

未运行全部README命令、外部链接、docs.rs渲染、远端CI或逐feature runtime tests。
