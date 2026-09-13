---
schema_version: 1
id: evidence.feature-table-doc-verification
kind: evidence
observed_at: 8ab20d1157c4e4fdeb3a805a32b5dcc3bc8324f5
source_refs:
  - Cargo.toml
  - README.md
  - README.zh.md
  - echo-agent-learning/tests/documentation_contract.rs
supports: [behavior.workspace-composition, rule.framework-layer-ownership]
limitations:
  - 完整workspace合并门禁与远程CI尚未执行
  - 未运行README中的所有feature组合
---

# README feature table 验证证据

## 支持的结论

新contract在旧README上稳定exit 101，两个失败都是`missing=[]`/`extra=["tasks"]`，没有缺失真实feature，也不是编译或环境假失败。

修复后focused feature contract通过1 test，完整documentation contract通过7 tests。Contract对root package缺失、当前feature集合意外改变、section/表行缺失、重复、真实feature缺失和多余feature fail closed。

## 来源与范围

Red/green日志位于`.supreme/logs/plan19-*`。源码diff只有双语README feature表/Task core说明与现有documentation contract。

## 已知缺口

Formatter、目标Clippy `-D warnings`、learning crate check、semantic gates、92/92 Finding-Issue对账和独立review均已通过；review的Critical、Important、Minor均为0。未执行完整workspace合并门禁、docs.rs渲染或远程CI。
