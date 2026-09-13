---
schema_version: 1
id: evidence.workspace-topology-doc-verification
kind: evidence
observed_at: source:1edd0f8dd43db91c544af47174e3f57154b9598d3bd78d9a3e7859a422f24a91
source_refs:
  - Cargo.toml
  - README.md
  - README.zh.md
  - echo-agent-learning/tests/documentation_contract.rs
supports: [behavior.workspace-composition, rule.framework-layer-ownership]
limitations:
  - 完整workspace合并门禁与远程CI尚未执行
  - 未验证Issue #79/#80对应的feature和example target合同
---

# Workspace topology 文档验证证据

## 支持的结论

新contract在旧README上稳定exit 101，精确报告英文/中文各缺`echo-sdk-protocol/`、`echo-sdk-host/`与Cargo-derived `8+2+1`分组摘要；不是编译失败或不相关失败。

修复后focused topology contract通过1 test，完整documentation contract通过6 tests。Contract在metadata不能执行、返回缺失package、manifest path没有parent/超出workspace、README缺少section/package或分组计数不匹配时fail closed。

## 来源与范围

Red/green与完整documentation logs位于`.supreme/logs/plan18-*`。源码diff只有双语README workspace/highlight和现有documentation contract。

## 已知缺口

Formatter、目标Clippy `-D warnings`、learning crate check、semantic gates、92/92 Finding-Issue对账和独立review均已通过；review的Critical、Important、Minor均为0。未执行完整workspace合并门禁、docs.rs渲染或远程CI。
