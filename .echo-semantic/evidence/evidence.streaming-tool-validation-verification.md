---
schema_version: 1
id: evidence.streaming-tool-validation-verification
kind: evidence
observed_at: source:aaf0d4c101710a5879fe6066820ffc3145ab93b4295ab8aa22878d54e7a050b5
source_refs:
  - echo-execution/src/tools.rs
supports: [behavior.effect-permission-execution, rule.permission-effect-order]
limitations:
  - 完整workspace合并门禁与远端CI尚未执行
  - 未执行所有第三方Tool validator或真实外部effect故障注入
---

# Streaming Tool validation 验证证据

## 支持的结论

旧实现的schema与custom stream测试均因非法参数到达Tool而exit 101；最初Box错误匹配导致的编译失败明确排除，不作为red。修复后25个ToolManager execute-with-context tests通过，覆盖新增两级stream validation以及既有cache、retry、timeout、cancel、drain和output forwarding。

## 来源与范围

`echo_execution` all-target Clippy以`-D warnings`通过，crate check通过；`contracts/sdk`与`sdks/shared`相对基准零diff。新增custom validator test分别观测validation一次与execution零次，schema test观测execution零次。

## 已知缺口

本证据不覆盖cache并发一致性、permission组合、sandbox failure typing或全部domain Tool实现。
