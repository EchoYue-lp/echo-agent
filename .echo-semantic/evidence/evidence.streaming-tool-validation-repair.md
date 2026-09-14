---
schema_version: 1
id: evidence.streaming-tool-validation-repair
kind: evidence
observed_at: 50890faac10ab91c90dc45769854c4b6e35f8376
source_refs:
  - echo-execution/src/tools.rs
  - echo-core/src/tools/mod.rs
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 不修复read cache scope或in-flight invalidation，也不改变permission与cancellation precedence
  - custom validator本身的外部副作用仍由Tool实现合同约束
---

# Streaming Tool validation 修复证据

## 支持的结论

基准`57066461ddbe8a32ce63f1b75dd40603530e786e`的non-stream执行与public validator依次执行JSON Schema和`Tool::validate_parameters`，stream inner则在取得Tool后直接进入cache、permit和stream future。真实schema/custom invalid测试确认旧stream路径仍执行Tool。

当前私有async `validate_tool_input`唯一拥有schema后custom的顺序；non-stream、stream和public validator均复用它。Stream在cancel检查、cache读写、permit、retry与Tool future之前完成校验，失败沿用既有typed error。

## 来源与范围

`echo-execution/src/tools.rs`包含ToolManager三个consumer与回归测试；`echo-core/src/tools/mod.rs`定义custom validation合同。没有新增public API、error、validator policy或permission owner。

## 已知缺口

本修复不改变validator本身可做的工作，也不处置cache identity/epoch、sandbox或Hook permission Finding。
