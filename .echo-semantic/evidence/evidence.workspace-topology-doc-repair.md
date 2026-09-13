---
schema_version: 1
id: evidence.workspace-topology-doc-repair
kind: evidence
observed_at: source:1edd0f8dd43db91c544af47174e3f57154b9598d3bd78d9a3e7859a422f24a91
source_refs:
  - Cargo.toml
  - README.md
  - README.zh.md
  - echo-agent-learning/tests/documentation_contract.rs
supports: [behavior.workspace-composition, rule.framework-layer-ownership]
limitations:
  - 只修复workspace package topology与分组，不修改feature表或example命令
  - Cargo manifests、runtime、public API与SDK inventory没有变化
---

# Workspace topology 文档修复证据

## 支持的结论

基准`9d1f3f2b5fdc204c08ecdec32ed22e8df95870e9`的Cargo metadata列出root package与10个workspace members，共11 package；旧双语README workspace tree漏掉`echo-sdk-protocol`/`echo-sdk-host`，并声称只有8 production crates + 1 teaching crate。

当前README tree显式列出两个SDK package，将workspace分为8个framework/runtime package、2个SDK package和1个learning package。Protocol package拥有deterministic facade inventory/contracts/code generation，Host package通过ACP和namespaced operations消费root `echo_agent` facade；learning package仍是不发布消费者。

`root_readmes_match_workspace_package_topology`直接执行`cargo metadata --no-deps --format-version 1 --locked`，以workspace member package ID和manifest path派生可检查目录与分组计数。测试不从README或第二份手写manifest建立expected topology。

## 来源与范围

修复仅改变root双语README与现有learning documentation contract。Cargo仍是package topology权威，README和test是consumer。

## 已知缺口

Issue #79的`tasks` feature漂移和Issue #80的`demo34_workflow_stream` target漂移仍保持open；本Evidence不证明所有README代码块、feature或外部链接正确。
