---
schema_version: 1
id: evidence.tokenizer-calibration-feedback-repair
kind: evidence
observed_at: 07e4270380c40df3f412f99c0aecb6145410cb2a
source_refs:
  - echo-core/src/tokenizer.rs
  - echo-state/src/compression/mod.rs
  - src/agent/react/run/context.rs
  - src/agent/react/run/phases/compact.rs
  - src/agent/react/run/phases/think.rs
  - docs/en/04-compression.md
  - docs/zh/04-compression.md
supports: [finding.tokenizer-calibration-feedback-convergence, behavior.llm-provider-execution, rule.provider-protocol-boundary]
limitations:
  - Provider-neutral estimates remain approximate, not a billing-grade model tokenizer
  - Image, file, and provider-specific reasoning requests do not update the text calibration factor without comparable usage decomposition
  - Remote CI and mainline delivery remain pending
---

# Tokenizer feedback and request budget authority repair

## 支持的结论

The ReAct think phase constructs one `ChatRequest` before budget admission and
retries clones of that same request. It derives an uncalibrated estimate from
the request's messages, visible tool definitions, and response-format schema;
the estimate paired with provider prompt usage is never the already-adjusted
`CalibratedTokenizer::count_tokens` result. Missing prompt usage and
provider-specific image, file, or reasoning payloads do not become calibration
samples. Cache-read and cache-creation tokens are included only when the
provider also supplies the prompt-token field.

Text and schema estimates use one factor snapshot per request. Fixed image
costs retain the existing `MessageContent::estimated_tokens` semantics rather
than being multiplied by the text factor. Context preparation, its
pre-compaction Draft-memory flush, and the final request budget all reserve
the current tool and response-format schema overhead. `ContextManager` uses
one input-budget calculation for the flush preflight and preparation, including
the no-percentage-budget branch. The final request remains authoritative if
tool visibility changes after pre-compaction.

## 来源与范围

`CalibratedTokenizer` owns the scalar text factor; the provider-normalized
`Usage` and immutable request are the feedback pair. `ContextManager` owns
the compression decision, while the ReAct caller supplies request overhead.
No second retry, tool-surface, or context-budget authority was added. The
change does not claim exact image token pricing or equivalent encrypted
reasoning replay across providers.

## 已知缺口

The integrated candidate passed independent incremental rereview, the full
workspace gate, and the independent feature matrix. Remote CI and mainline
delivery remain pending.
