---
schema_version: 1
id: evidence.structured-output-main-path-verification
kind: evidence
observed_at: 07e4270380c40df3f412f99c0aecb6145410cb2a
source_refs:
  - src/agent/react/run/stream_channel.rs
  - src/testing/mock_llm.rs
  - src/agent/react/run/phases/think.rs
supports: [finding.structured-output-main-path]
limitations:
  - Remote CI and mainline delivery remain pending
command_results:
  - { command: "cargo test -p echo_agent configured_response_format_reaches_every_react_request --lib --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent unknown_model_does_not_silently_enable_structured_output --lib --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent explicit_text_format_keeps_anthropic_request_unconstrained --lib --locked", exit_code: 0 }
  - { command: "./scripts/verify.sh", exit_code: 0 }
  - { command: "17 independent-feature cargo checks", exit_code: 0 }
---

# Structured output main request verification

## 支持的结论

The real ReAct caller test records the declared JSON request on both a
tool-using and final model call. Unknown model facts produce an error without
calling the LLM. Explicit Text on an Anthropic-profile Agent retains the
unconstrained request while finishing normally.

## 来源与范围

These focused tests were run after the request snapshot wiring and Text
normalization. They inspect actual `MockLlmClient` ChatRequest fields rather
than builder configuration alone. The final integrated stream suite later
passed 82/82 after Guard and schema terminal reconciliation.

## 已知缺口

The final integrated source passed the complete workspace/all-feature gate
and 17 independent-feature checks. Remote Linux/Windows and mainline checks
remain to be executed on the PR and final merge commit.
