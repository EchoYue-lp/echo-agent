---
schema_version: 1
id: evidence.readme-example-target-verification
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
  - 完整workspace合并门禁与远程CI尚未执行
  - 其付README命令只做target存在性检查，未全部运行
---

# README example target 验证证据

## 支持的结论

新contract在旧README上稳定exit 101，只报告中英文各自引用不存在的`demo34_workflow_stream` learning example target；其余8条quick learning命令均能与Cargo metadata匹配。

修复后focused command contract通过1 test。修正后README原样命令已真实执行，`example_contracts` binary内的`contract_demo34_workflow_stream` 1 test通过。Contract对shell parse、package action、`--example`/`--test` target、Cargo target kind、`example_contracts`唯一filter和filter源码定义fail closed。

## 来源与范围

Red/green与真实README command日志位于`.supreme/logs/plan20-*`。源码diff只有双语README demo34命令和现有documentation contract。

## 已知缺口

完整documentation contract 8、README原样demo34 contract 1、formatter、目标Clippy `-D warnings`、learning crate check、semantic gates、92/92 Finding-Issue对账和独立review已通过；最终review的Critical、Important、Minor均为0。未执行完整workspace合并门禁、docs.rs渲染或远程CI。
