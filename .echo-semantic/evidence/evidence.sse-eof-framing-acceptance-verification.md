---
schema_version: 1
id: evidence.sse-eof-framing-acceptance-verification
kind: evidence
observed_at: e842d87bb787fb0b1fd39afbc04f834df07aec84
source_refs:
  - echo-integration/src/providers/client.rs
  - echo-integration/src/providers/openai.rs
  - echo-integration/src/providers/responses.rs
  - echo-integration/src/providers/anthropic.rs
supports: [behavior.llm-provider-execution, rule.provider-protocol-boundary]
limitations:
  - 完整workspace门禁与远端CI留到汇总MR前执行
  - 测试使用本地字节流与parked stream，不调用真实provider网络
---

# SSE EOF framing验证证据

## 支持的结论

定向测试覆盖delimiterless完整JSON、纯空白残余、合法LF/CRLF终止事件、拆分UTF-8和
parked stream cancellation。独立reviewer同时沿OpenAI、Responses、Anthropic真实入口
确认三者均使用同一decoder，且取消、`[DONE]`和provider semantic terminal归约未被改动。

## 来源与范围

复审绑定commit `e842d87bb787fb0b1fd39afbc04f834df07aec84`及其tree，排除其它并行
provider cancellation和当前未提交SDK文件。结论为pass，Critical、Important、Minor均为0。

## 已知缺口

完整all-feature、no-default、feature matrix与远端Linux/Windows信号由最终MR门禁统一验证。
