---
schema_version: 1
id: evidence.shared-tool-input-validation-candidate
kind: evidence
observed_at: 24e3c94e9b00619a982092f95f4d923b6e44b089
source_refs:
- echo-execution/src/tools.rs
- src/tools/mod.rs
- docs/adr/0083-shared-tool-input-validation.md
- docs/en/02-tools.md
- docs/zh/02-tools.md
supports:
- behavior.effect-permission-execution
- rule.permission-effect-order
limitations:
- Visibility and documentation of existing validator; no separate validation or execution authority.
- New final23, independent complete repair evidence and published framework/SDK main remain required; historical
  b0d162 gates do not cover this delta.
---


# 直接适配器复用唯一工具输入校验

## 支持的结论

原 validate_tool_input 实现保持，ToolManager 原调用不变；public入口供直接适配器复用同工具schema与custom validator，无新增executor/timeout/retry/registry/permission。SDK只有positional绑定与错误转换，不复制schema校验。

## 来源与范围

ADR0083记录MCP官方Tool.run调用既有argument validator和OpenAI官方typed function schema共性；SDK真实旧unknown chart type静默line，新Host通过同框架validator返回canonicalToolInvalidParameter，合法bar相同。SDK临时组合完整418tests通过；其原始receipt另归SDK，不冒框架门禁完成。host_actor_delta_review已只读源码分层审核pass，同时要求正式invalidtype/required/extra-null副作用反例和custom调用证据。

## 已知缺口

新框架23正在执行；正式SDK负例及新快照Clippy/三语言集中门禁待齐，source-level pass不替代运行证明。只暴露Rust既有helper，不自动接受跨语言identity；框架examples和双语文档纳入适用门禁，官网投影待公开revision。


## 历史快照定位

原 source:ecac04395732966d09b9406f0485df8b1137ecbd2c1b4a1cf29198930f0bfaee 的847个真实文件已由可恢复 Git 24e3c94e9b00619a982092f95f4d923b6e44b089 保存；本次用唯一校验器重算摘要相同。原验证时间、命令、限制保持，不作为新增 Guard/Pivot/Channel 的完整门禁证明。
