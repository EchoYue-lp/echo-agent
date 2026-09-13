---
schema_version: 1
id: map.llm-provider-runtime
kind: capability_map
title: LLM Provider、Model Policy 与 Streaming
risk: high
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
boundary_refs: [boundary.llm-provider-runtime]
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.high-risk-audit-frontier]
finding_refs: [finding.structured-output-main-path, finding.structured-output-schema-validation-contract, finding.provider-capability-authority, finding.model-fact-freshness-authority, finding.nonstream-cancellation-parity, finding.sse-eof-framing-acceptance, finding.provider-stream-terminal-parity, finding.tokenizer-calibration-feedback-convergence]
audit_refs: [audit.llm-provider-runtime.contract-evidence, audit.llm-provider-runtime.failure-concurrency, audit.llm-provider-runtime.time-lifecycle]
related_map_refs: [map.agent-session-turn, map.context-memory, map.protocol-surfaces, map.eval-evolution]
scenarios:
  provider-neutral-request:
    status: mapped
    source_refs: [echo-core/src/llm/mod.rs, echo-core/src/llm/types.rs]
    behavior_refs: [behavior.llm-provider-execution]
    rule_refs: [rule.provider-protocol-boundary]
  model-budget-tokenizer-policy:
    status: needs_review
    source_refs: [echo-core/src/llm/capabilities.rs, echo-core/src/budget.rs, echo-core/src/tokenizer.rs]
    finding_refs: [finding.provider-capability-authority, finding.model-fact-freshness-authority, finding.tokenizer-calibration-feedback-convergence]
    evidence_refs: [evidence.provider-protocol-quality]
    unknown: config/profile capability、动态 model facts 与生产 calibration feedback 尚未形成单一权威
    next_step: semantic-decide capability/freshness owner，并独立 repair calibration
  stream-transport-and-terminal:
    status: needs_review
    source_refs: [echo-integration/src/providers/client.rs, echo-integration/src/providers/responses.rs]
    behavior_refs: [behavior.llm-provider-execution]
    evidence_refs: [evidence.provider-protocol-quality]
    finding_refs: [finding.sse-eof-framing-acceptance, finding.provider-stream-terminal-parity]
    unknown: shared SSE framing 与 provider-specific semantic terminal 在 EOF/截断路径不对等
    next_step: repair framing 与每 provider terminal state machine 并补失败矩阵
  structured-output:
    status: mapped
    source_refs: [src/agent/react/builder.rs, src/agent/react/run/phases/think.rs, src/agent/react/extract.rs]
    finding_refs: [finding.structured-output-main-path, finding.structured-output-schema-validation-contract]
  nonstream-cancellation:
    status: mapped
    source_refs: [echo-integration/src/providers/openai.rs, echo-integration/src/providers/anthropic.rs, echo-integration/src/providers/responses.rs]
    finding_refs: [finding.nonstream-cancellation-parity]
  changing-model-facts:
    status: needs_review
    source_refs: [echo-core/src/llm/capabilities.rs, echo-integration/src/providers/config.rs]
    unknown: 快速变化的模型 window/modalities/tool 行为不能由静态内置 catalog 长期保证
    finding_refs: [finding.provider-capability-authority, finding.model-fact-freshness-authority]
    next_step: semantic-decide 明确 framework 默认、provider 声明和应用 override 的 precedence 与刷新责任
---

# LLM Provider、Model Policy 与 Streaming

## 能力范围

覆盖 LlmClient/request/response/chunk、provider config/adapters、ModelProfile、budget/tokenizer、timeout、stream 和 structured output。

## 入口与输出

Agent think/summary/grader 与 direct caller 发出 provider-neutral request；输出 response/stream、usage、typed error 和 trace event。

## 行为关系

Core 定义合同，integration 翻译 wire，ModelProfile/budget/tokenizer 决定 harness；provider adapter 不拥有业务 terminal。

## 状态与数据流

Config 构造 client，profile 解析 capability，request 携带 timeout/cancel/format，stream decoder 归约 chunk/terminal，usage 回灌 tokenizer。

## 策略来源与优先级

Explicit per-request override、configured model profile、provider declaration 与保守 default 需形成唯一合成顺序。

## 生命周期与失败路径

Build/request/first chunk/idle/overall/finish/cancel；malformed/truncated/timeout/rate-limit 不得静默完成。

## 权限与敏感信息

Credential 只应进入 HTTP headers/config secret；trace、diagnostic 与 SDK contract 的 redaction/retention 必须由具体 producer/backend 证明，不能作全局推断。

## 用户侧投影

Token/usage/thinking/structured result 是 provider-neutral projection；快速变化事实由消费方配置。

## 场景处置清单

核心 request/stream 路由已映射，八个当前问题进入 Finding；structured output、capability owner 与动态 model facts 保持 needs_review/待裁决。

## 未展开项

具体 provider 的完整 payload fixture 在下一阶段 audit 执行。
