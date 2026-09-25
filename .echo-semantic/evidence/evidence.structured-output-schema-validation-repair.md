---
schema_version: 1
id: evidence.structured-output-schema-validation-repair
kind: evidence
observed_at: source:5e9a01f48cdf8338bbf8ccfdf225290e1c3f3db90fbea6c30cf1219b92cd1f48
source_refs:
  - echo-core/src/error.rs
  - src/agent/react/extract.rs
  - src/agent/react/structured.rs
  - src/agent/react/run/phases/mod.rs
  - src/agent/react/run/phases/verify.rs
  - src/agent/react/run/phases/tools.rs
  - src/agent/react/run/phases/finalize.rs
  - src/agent/react/run/stream_channel.rs
  - src/agent/critic/llm_critic.rs
  - docs/adr/0079-strict-structured-output-validation.md
supports: [finding.structured-output-schema-validation-contract, behavior.llm-provider-execution, rule.provider-protocol-boundary]
limitations:
  - External HTTP/file schema references are rejected; callers must bundle local definitions
  - Provider streaming tokens and tool-result events remain provisional before final validation
  - Remote CI and mainline delivery remain pending
---

# Strict structured output validation repair

## 支持的结论

One prepared JSON Schema validator checks strict output locally. One-shot
extraction, main ReAct text, ToolOutput-guarded `final_answer`, and the
LlmCritic strict-hint success branch cannot accept a schema-invalid value.
Invalid JSON and schema mismatch produce typed, response-content-free errors
and bounded correction attempts. `strict: false` remains a provider hint;
`JsonObject` checks syntax without enforcing schema shape.

## 来源与范围

For text, accepted steer is drained, Output Guard transforms once, then the
framework validates the authoritative answer before Critic and final
observers. For tools, all results are projected and durably settled before
the run driver drains steer, fences cancellation, selects the last schema-
and Critic-accepted answer, and publishes a success. A waiting Critic is
cancel-aware. Failed or cancelled text remains outside the user-visible
assistant transcript until Stop and final intervention permit success.

ADR 0079 records local `$defs` and external-reference rejection, schema
retry authority, full tool-batch settlement, and final callback safe points.

## 已知缺口

These are framework boundaries, not EKO product policy. The complete gate
and feature matrix passed on the final integrated source; remote CI and
post-merge semantic verification remain separate acceptance steps.
