# ADR 0078: Main ReAct structured-output request authority

## Background

`ReactAgentBuilder::response_format` and `output_type` set `AgentConfig`, but the
run snapshot did not retain that format and the main ReAct request always sent
`response_format: None`. A one-shot extraction request had a different path, so
its provider hint did not repair the normal Agent execution contract.

ADR 0045 DU-68 requires conservative model capability resolution. DU-97 requires
framework-local validation for strict JSON Schema; a provider hint alone cannot
establish that guarantee.

## Options

1. Leave the format in `AgentConfig` and let each provider infer it from prompts.
   This preserves the silent omission in the main request.
2. Copy the configured format into the immutable run snapshot and send it on
   each ReAct request, rejecting a JSON format when resolved model facts do not
   affirm structured-output support.
3. Disable tools whenever a response format is configured. This would change
   the Agent's tool contract and make the format option remove capabilities.

## Decision

Use option 2. The caller-declared format and the resolved model capability are
captured together for each run. Every model request uses that snapshot. JSON
formats fail before the model call when the resolved capability is false or
unknown. An explicit `Text` format is normalized to the protocol's default
unconstrained request (`None`), so providers that do not implement a
`response_format` wire field remain compatible.
Tool calls remain available. Tool arguments are not treated as final answers;
DU-97's local validation must cover both final text and `final_answer` tool
completion before either is reported as a strict success.

The [OpenAI Structured Outputs cookbook](https://cookbook.openai.com/examples/structured_outputs_intro)
distinguishes response-format schemas from strict function schemas and treats
refusals separately from schema-conforming responses. This supports keeping the
provider hint and framework final-result validation as separate responsibilities.

## Consequences

- `response_format` is no longer silently discarded by the primary ReAct path.
- Unknown or stale provider/model facts cannot elevate JSON response capability.
- A provider can still reject a format it does not implement; the framework
  propagates that error rather than silently retrying without the format.
- ADR 0079 defines the separate framework-local validation and bounded repair
  authority required by DU-97. Provider hints alone never establish success.
