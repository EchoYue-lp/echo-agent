---
schema_version: 1
id: audit.readme-example-target-rereview
kind: audit
boundary_ref: boundary.workspace-architecture
lens: contract_evidence
freshness: examined
revision: 7ba1f12279a65296257bda79206ccd9dd0e078a2
finding_refs: [finding.readme-example-target-drift]
challenges:
  cargo-target-kind-and-command-parse:
    revision: 7ba1f12279a65296257bda79206ccd9dd0e078a2
    source_refs: [echo-agent-learning/Cargo.toml, echo-agent-learning/tests/documentation_contract.rs, README.md, README.zh.md]
    evidence_refs: [evidence.workspace-structure, evidence.readme-example-target-repair, evidence.readme-example-target-verification]
  contract-filter-and-real-execution:
    revision: 7ba1f12279a65296257bda79206ccd9dd0e078a2
    source_refs: [echo-agent-learning/tests/example_contracts.rs, echo-agent-learning/tests/example_contracts/demo34_workflow_stream.rs]
    evidence_refs: [evidence.readme-example-target-repair, evidence.readme-example-target-verification]
  bilingual-and-adjacent-scope:
    revision: 7ba1f12279a65296257bda79206ccd9dd0e078a2
    source_refs: [README.md, README.zh.md, .echo-semantic/findings/finding.workspace-topology-doc-drift.md, .echo-semantic/findings/finding.public-feature-table-drift.md]
    evidence_refs: [evidence.readme-example-target-verification]
---

# README example target 独立复审

## 审查范围

复审Cargo metadata target name/kind、shlex命令解析、run/example与test/test路由、`example_contracts` filter存在性、README原样命令执行，双语对等，#114/#79回归隔离，语义Evidence与Issue状态。

## 已检查故障假设

验证metadata是否把test当example，shlex是否把尾部注释当参数，缺package/action/target是否fail closed，`example_contracts`是否允许无filter、多filter或不存在filter，中英命令是否同路由，静态校验通过后实际Cargo命令是否仍失败，以及已闭合topology/feature是否被回退。

## 实际实现路径与证据

Contract从`echo-agent-learning` metadata targets派生example/test集合，用`shlex`解析单行learning Cargo命令。`cargo run`必须引用真实`--example`，`cargo test`必须引用真实`--test`；`example_contracts`还必须有唯一`contract_*` token且对应源码函数。双语README的demo34现指向`example_contracts/contract_demo34_workflow_stream`。

旧README上red只报告两条missing demo34 example target；修复后focused command contract 1、documentation contract 8和README原样demo34 command 1测试通过。Formatter、目标Clippy `-D warnings`、learning crate check、manifest/example/SDK路径零diff、semantic strict/change-evidence通过。

## 问题记录

首轮独立review只发现1个Evidence错别字Minor，修正后最终review为PASS，Critical、Important、Minor均为0。`finding.readme-example-target-drift`具备repair、verification与rereview证据，可标记resolved。Issue #80保持open，等待本地提交进入远程main后关闭。

## 残余风险

静态contract只覆盖当前README的单行`-p/--package` learning命令形态，不递归执行所有README命令；本次修正命令有单独真实执行证据。

## 未检查项

未运行其余README命令的外部provider/网络场景、docs.rs渲染、完整workspace合并门禁或远端CI。
