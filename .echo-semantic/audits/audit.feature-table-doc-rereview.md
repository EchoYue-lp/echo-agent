---
schema_version: 1
id: audit.feature-table-doc-rereview
kind: audit
boundary_ref: boundary.workspace-architecture
lens: contract_evidence
freshness: examined
revision: 8ab20d1157c4e4fdeb3a805a32b5dcc3bc8324f5
finding_refs: [finding.public-feature-table-drift]
challenges:
  root-feature-authority:
    revision: 8ab20d1157c4e4fdeb3a805a32b5dcc3bc8324f5
    source_refs: [Cargo.toml, echo-agent-learning/tests/documentation_contract.rs]
    evidence_refs: [evidence.workspace-structure, evidence.feature-table-doc-repair, evidence.feature-table-doc-verification]
  bilingual-exact-set-table:
    revision: 8ab20d1157c4e4fdeb3a805a32b5dcc3bc8324f5
    source_refs: [README.md, README.zh.md, Cargo.toml]
    evidence_refs: [evidence.feature-table-doc-repair, evidence.feature-table-doc-verification]
  task-core-and-adjacent-scope:
    revision: 8ab20d1157c4e4fdeb3a805a32b5dcc3bc8324f5
    source_refs: [README.md, README.zh.md, .echo-semantic/findings/finding.readme-example-target-drift.md]
    evidence_refs: [evidence.feature-table-doc-verification]
---

# README feature table 独立复审

## 审查范围

复审root Cargo package选择、metadata feature set派生、双语Feature Flags section与表格提取、default/full/tasks边界、Task core说明，#80范围隔离，语义Evidence与Issue状态。

## 已检查故障假设

验证root package是否通过名称猜测，`default`是否被错列为可启用能力，`full`是否丢失，表格提取是否跨到Feature Matrix或后续章节，空表/重复/缺失/多余是否fail closed，双语是否只删除`tasks`，以及demo34错误命令是否被偷偷修改。

## 实际实现路径与证据

Contract以`manifest_path == workspace_root/Cargo.toml`选择root package，从结构化`features` map的28个key排除`default`后得到27个expected row。表格提取限定于英文`### Feature Flags`与中文`## Feature Flags`，并检查空集、重复、missing和extra。双语README只删除`tasks`行，就近说明Task API属framework core。

旧README上red只报告中英文`missing=[]`/`extra=["tasks"]`；修复后focused 1与documentation contract 7测试通过。Formatter、目标Clippy `-D warnings`、learning crate check、Cargo/examples/SDK路径零diff、semantic strict/change-evidence通过。

## 问题记录

独立review的Critical、Important、Minor均为0。`finding.public-feature-table-drift`具备repair、verification与rereview证据，可标记resolved。Issue #79保持open，等待本地提交进入远程main后关闭。

## 残余风险

Contract只验证feature名称集合，不解析描述/依赖列或`default`展开内容；这与本Finding的公开名称漂移范围一致。

## 未检查项

未运行所有feature组合、README命令、docs.rs渲染、完整workspace合并门禁或远程CI；#80保持open delivery outcome。
