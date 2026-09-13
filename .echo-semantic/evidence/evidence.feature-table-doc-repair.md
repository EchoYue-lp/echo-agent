---
schema_version: 1
id: evidence.feature-table-doc-repair
kind: evidence
observed_at: 8ab20d1157c4e4fdeb3a805a32b5dcc3bc8324f5
source_refs:
  - Cargo.toml
  - README.md
  - README.zh.md
  - echo-agent-learning/tests/documentation_contract.rs
supports: [behavior.workspace-composition, rule.framework-layer-ownership]
limitations:
  - 只修复root feature表与Task core说明，不修改example命令
  - 不校验每个feature的自然语言描述与dependency列
---

# README feature table 修复证据

## 支持的结论

基准`53accbac880639a58a639a56f25c72dc36c90ca4`的root `echo_agent` metadata包含28个feature key，排除空`default`后有27个可列公开feature；其中没有`tasks`，Cargo.toml同时声明Task API属于framework core。

当前双语README feature表已删除不存在的`tasks`行，并在表前就近说明Task API没有独立feature。其他27个真实feature的现有顺序与描述保持不变。

`root_readme_feature_tables_match_cargo_metadata`复用结构化Cargo metadata，只选root package的`features` map keys，并将英文`### Feature Flags`与中文`## Feature Flags`表格第一列与该集合比较。

## 来源与范围

修复只改变root双语README和现有documentation contract。没有新增Cargo feature alias、依赖、runtime/API或SDK identity。

## 已知缺口

Issue #80的`demo34_workflow_stream` target漂移仍保持open。Feature描述列和可选dependency的全量语义校验不在本Evidence范围。
