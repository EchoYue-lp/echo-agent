---
schema_version: 1
id: evidence.nonstream-cancellation-parity-repair
kind: evidence
observed_at: 13a6a742a4eec2dcc971945d9887662310c3ce43
source_refs:
  - echo-core/src/llm/mod.rs
  - echo-integration/src/providers/client.rs
  - echo-integration/src/providers/openai.rs
  - echo-integration/src/providers/responses.rs
  - echo-integration/src/providers/anthropic.rs
supports: [behavior.llm-provider-execution, rule.provider-protocol-boundary]
limitations:
  - provider stream semantic terminal parity仍由finding.provider-stream-terminal-parity追踪
  - 本修复不新增LlmError variant或改变request timeout配置
---

# Non-stream provider cancellation修复证据

## 支持的结论

OpenAI Chat、Responses与Anthropic non-stream请求统一进入`post_json_request`。同一取消
token覆盖request send、response headers、错误/成功body读取、严格bytes JSON decode以及
provider-specific projection返回边界。取消映射到既有typed LLM error，不伪造成成功。

JSON decode使用8 KiB cooperative reader；取消时未启动的blocking task被abort，已启动任务
协作停止并被await，避免后台decode越过调用方terminal。Response body不经lossy text
转换，非法UTF-8保持InvalidResponse；日志只记录长度。

## 来源与范围

最终实现由汇总分支commit `13a6a742a4eec2dcc971945d9887662310c3ce43`承载，复用
既有ChatRequest cancellation token和provider client helper，不创建第二状态机。

## 已知缺口

本证据不证明streaming provider semantic terminal或SSE framing；后者由#95单独修复，
前者继续由#78跟踪。
