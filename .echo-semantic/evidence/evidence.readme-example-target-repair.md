---
schema_version: 1
id: evidence.readme-example-target-repair
kind: evidence
observed_at: source:e5875d2d355e903b53de984f1f399ba10a08d50a8d5d42a03418a2264c09f3b9
source_refs:
  - README.md
  - README.zh.md
  - echo-agent-learning/Cargo.toml
  - echo-agent-learning/tests/documentation_contract.rs
  - echo-agent-learning/tests/example_contracts.rs
  - echo-agent-learning/tests/example_contracts/demo34_workflow_stream.rs
supports: [behavior.workspace-composition, rule.framework-layer-ownership]
limitations:
  - 只修复root README的learning command target/filter，不修改example源码
  - 静态contract不执行README中的每条命令
---

# README example target 修复证据

## 支持的结论

基准`8ab20d1157c4e4fdeb3a805a32b5dcc3bc8324f5`的Cargo metadata中，`echo-agent-learning`有`example_contracts` test target，但没有`demo34_workflow_stream` example target。`contract_demo34_workflow_stream`是`tests/example_contracts/demo34_workflow_stream.rs`中的真实test，并由`tests/example_contracts.rs`编译进该test binary。

当前双语README quick examples已将demo34命令改为`cargo test -p echo-agent-learning --test example_contracts --all-features --locked contract_demo34_workflow_stream`，不新增平行example wrapper。

`root_readme_learning_commands_reference_cargo_targets`复用Cargo metadata中learning package的target name/kind，通过`shlex`解析root README中的learning cargo命令，并验证`example_contracts`命令的唯一`contract_*` filter在真实源码中定义。

## 来源与范围

修复只改变root双语README的两条demo34命令与现有documentation contract。Cargo manifests、examples、runtime/API与SDK inventory没有变化。

## 已知缺口

Contract只验证root README中`-p echo-agent-learning`的run/test路由和contract filter；不证明外部provider、feature运行环境或所有README命令成功。
