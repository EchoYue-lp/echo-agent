---
schema_version: 1
id: audit.llm-provider-runtime.time-lifecycle
kind: audit
boundary_ref: boundary.llm-provider-runtime
lens: time_lifecycle
freshness: examined
revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
finding_refs: [finding.nonstream-cancellation-parity, finding.provider-capability-authority, finding.provider-stream-terminal-parity, finding.tokenizer-calibration-feedback-convergence]
challenges:
  timeout-and-terminal:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-integration/src/providers/client.rs, echo-integration/src/providers/responses.rs, echo-integration/src/providers/anthropic.rs, src/agent/react/run/phases/think.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  timeout-override-precedence:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-core/src/llm/mod.rs, echo-integration/src/providers/config.rs, echo-integration/src/providers/openai.rs, echo-integration/src/providers/responses.rs, echo-integration/src/providers/anthropic.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  tokenizer-feedback:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-core/src/tokenizer.rs, src/agent/react/run/phases/think.rs, tests/react_smoke.rs]
    evidence_refs: [evidence.provider-protocol-quality]
---

# LLM Timeout、Terminal、Usage 与 Calibration 生命周期审计

## 审查范围

审查 typed first/idle/overall timeouts、request override、semantic terminal、usage 回灌与 tokenizer calibration。

## 已检查故障假设

验证 timeout 是否从 request start 覆盖 body stall、Responses terminal 是否拒绝截断、request override 顺序是否一致，以及生产 calibration 是否收敛。

## 实际实现路径与证据

First/overall 从发送前建立，idle 覆盖 byte gap，stream cancel 覆盖启动和 body；Responses 拒绝缺 completed。三 provider 一致采用 request timeouts 整对象覆盖 client default。Anthropic body non-stream cancel 和 message_stop terminal 仍有缺口。生产把已乘当前 factor 的 estimate 再交给 calibrate，EMA 把 actual/adjusted 当绝对 factor，比例为 2 时趋向 sqrt(2)；且 estimate 未含 tool schema。

## 问题记录

确认 cancellation/capability/terminal Findings；新增 tokenizer calibration feedback convergence。

## 残余风险

极大反序列化 timeout 可能使 Instant 加法溢出；with_client 可注入隐藏 reqwest total timeout，保留为 residual。

## 未检查项

未做极值 timeout、hidden client timeout 或 live provider usage 校准测试。
