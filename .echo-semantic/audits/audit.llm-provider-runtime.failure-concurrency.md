---
schema_version: 1
id: audit.llm-provider-runtime.failure-concurrency
kind: audit
boundary_ref: boundary.llm-provider-runtime
lens: failure_concurrency
freshness: examined
revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
finding_refs: [finding.nonstream-cancellation-parity, finding.sse-eof-framing-acceptance, finding.provider-stream-terminal-parity]
challenges:
  nonstream-cancellation:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-integration/src/providers/client.rs, echo-integration/src/providers/openai.rs, echo-integration/src/providers/responses.rs, echo-integration/src/providers/anthropic.rs, echo-state/src/compression/compressor/summary.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  sse-eof-framing:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-integration/src/providers/client.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  provider-semantic-terminal:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-integration/src/providers/responses.rs, echo-integration/src/providers/openai.rs, echo-integration/src/providers/anthropic.rs, src/agent/react/run/phases/think.rs]
    evidence_refs: [evidence.provider-protocol-quality]
---

# LLM Cancellation、SSE Framing 与 Semantic Terminal 审计

## 审查范围

审查三 provider non-stream cancellation、共享 SSE EOF framing 与 Responses/OpenAI/Anthropic semantic terminal。

## 已检查故障假设

验证 cancellation 是否覆盖 send+body，缺事件分隔符的 EOF 是否被接受，以及 adapter 是否在成功前要求 provider-specific terminal。

## 实际实现路径与证据

OpenAI/Responses non-stream 完全不消费 cancel token；Anthropic 只在响应头前 select，body json 读取不可取消。SseDecoder finish 把缺空行边界的完整 JSON buffer 当事件返回。Responses 要求 response.completed；OpenAI EOF 不要求 DONE/finish，Anthropic 不建模 message_stop 并可在 message_delta 后接受截断。

## 问题记录

扩大 non-stream cancellation Finding；新增 SSE EOF framing 与 provider stream terminal parity。

## 残余风险

启动失败 retry 可能重复远端推理/计费，首字节后不 retry 的策略也缺正式合同；caller drop 无法证明远端立即停止。

## 未检查项

未运行 live provider 或分阶段 cancellation mock，未覆盖 delimiterless EOF、OpenAI/Anthropic terminal 和 caller-drop billing。
