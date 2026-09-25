---
schema_version: 1
id: evidence.structured-output-main-path-repair
kind: evidence
observed_at: source:bd676c53ff6a431d3ef88581ad7a92a4db718dad7e88dd872b64e20170f61b93
source_refs:
  - src/agent/config.rs
  - src/agent/snapshot.rs
  - src/agent/react/run/phases/think.rs
  - src/testing/mock_llm.rs
  - docs/adr/0078-structured-output-main-request.md
supports: [finding.structured-output-main-path, behavior.llm-provider-execution, rule.provider-protocol-boundary]
limitations:
  - Provider hint and capability admission do not replace framework-local strict schema validation; that result is owned by Issue 97 evidence
  - Full workspace gate, remote CI, and mainline delivery remain pending
---

# Structured output main request repair

## 支持的结论

The immutable ReAct run snapshot carries the caller's `response_format` and
fresh resolved structured-output capability. Every primary ReAct ChatRequest,
including retries and requests after a tool call, receives the JSON format.
Unknown or unsupported model facts reject JSON formats before invoking the
model; explicit `Text` remains an unconstrained wire request so Anthropic's
unsupported response-format field is not sent.

## 来源与范围

The existing ModelProfileResolver remains the capability authority. The
MockLlmClient records the actual format seen by the request boundary, and
the same `run_core_loop` serves streaming and non-streaming Agent entries.
ADR 0078 records the framework/provider division of responsibility.

## 已知缺口

This evidence alone does not close strict schema validation or prove real
provider acceptance. The local integration, complete gate, remote CI, and
mainline Issue closure are separate evidence steps.
