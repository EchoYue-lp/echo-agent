---
schema_version: 1
id: audit.workspace-topology-doc-rereview
kind: audit
boundary_ref: boundary.workspace-architecture
lens: contract_evidence
freshness: examined
revision: 53accbac880639a58a639a56f25c72dc36c90ca4
finding_refs: [finding.workspace-topology-doc-drift]
challenges:
  cargo-derived-package-closure:
    revision: 53accbac880639a58a639a56f25c72dc36c90ca4
    source_refs: [Cargo.toml, echo-agent-learning/tests/documentation_contract.rs]
    evidence_refs: [evidence.workspace-structure, evidence.workspace-topology-doc-repair, evidence.workspace-topology-doc-verification]
  bilingual-topology-and-classification:
    revision: 53accbac880639a58a639a56f25c72dc36c90ca4
    source_refs: [README.md, README.zh.md, Cargo.toml]
    evidence_refs: [evidence.workspace-topology-doc-repair, evidence.workspace-topology-doc-verification]
  adjacent-finding-isolation:
    revision: 53accbac880639a58a639a56f25c72dc36c90ca4
    source_refs: [README.md, README.zh.md, .echo-semantic/findings/finding.public-feature-table-drift.md, .echo-semantic/findings/finding.readme-example-target-drift.md]
    evidence_refs: [evidence.workspace-topology-doc-verification]
---

# Workspace topology 文档独立复审

## 审查范围

复审Cargo metadata package/member/manifest path派生，双语README workspace tree、package分类与责任，documentation contract的fail-closed边界，#79/#80范围隔离，语义Evidence和Issue状态。

## 已检查故障假设

验证contract是否从README自证计数，metadata执行/解析失败是否被吞掉，workspace member是否可与package不完整匹配，manifest path越界是否被忽略，package在section中缺失/重复是否失败，8/2/1分类是否与实际DAG一致，双语责任是否对等，以及本修复是否偷偷改动feature表或demo34命令。

## 实际实现路径与证据

Contract通过`cargo metadata --no-deps --format-version 1 --locked`读取结构化package/member/manifest数据，对执行、JSON、member匹配、数量、分类、path越界、section、缺失与重复都fail closed。双语README现在各自唯一列出10个非root package目录，并声明8个framework/runtime、2个SDK、1个learning package；protocol、Host与learning责任与Cargo DAG一致。

旧README上red精确报告中英文各缺两个SDK package与分组摘要；修复后focused 1与documentation contract 6测试通过。Formatter、目标Clippy `-D warnings`、learning crate check、SDK路径零diff、semantic strict/change-evidence通过。

## 问题记录

独立review的Critical、Important、Minor均为0。`finding.workspace-topology-doc-drift`具备repair、verification与rereview证据，可标记resolved。Issue #114保持open，等待本地提交进入远程main后关闭。

## 残余风险

Contract有意冻结当前11和8/2/1数量，未来合法新增package必须同步更新分类合同。Package责任prose与完整dependency DAG仍依赖review，presence/count测试不完全解析自然语言。

## 未检查项

未运行所有README命令、外部链接、docs.rs渲染、完整workspace合并门禁或远程CI；#79/#80保持各自open delivery outcome。
