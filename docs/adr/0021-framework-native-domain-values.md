# ADR 0021: Framework-Native Domain Values

## Status

Accepted

## Context

EKO had a `PermissionRuleConfig` that mirrored the framework's
`PermissionRule`, plus a `to_framework_rule` conversion helper. The same design
pressure previously appeared in delivery and Subagent result paths: a generic
framework value was copied into an application DTO and then reconstructed at a
runtime boundary. This makes the SDK look incomplete and gives each consumer a
second authority to keep in sync.

The official OpenAI Agents SDK exposes shared run facts directly on its
`RunResultBase`, and the Claude Agent SDK carries usage on its result message.
Both avoid source-named conversion APIs between execution result variants:
[OpenAI results](https://openai.github.io/openai-agents-python/results/) and
[Claude Agent SDK types](https://platform.claude.com/docs/en/agent-sdk/typescript).

## Decision

Product-neutral domain values stay in `echo-agent` and are used directly by
applications:

- `ConfigState` stores framework `PermissionRule` values, not an EKO copy.
- `RuleMatcher`, `RuleBehavior`, and `RuleSource` implement `FromStr` for
  validated command/UI spellings.
- `PermissionMode` owns its canonical kebab-case `id`, serde aliases, and
  `FromStr` parser, so applications do not need a mode DTO just to normalize
  transport input.
- `ExecutionUsage` is the reusable persistence/reporting shape for duration,
  tokens, and iterations. `SubagentResult::usage()` and `TurnReceipt::usage()`
  expose it directly from both execution result surfaces.
- `TurnReceipt` is the canonical finite-turn result. Consumers should retain it
  directly; display rounding, truncation, and product policy belong at the
  final surface boundary rather than in a second turn-outcome DTO.
- Tauri and other surfaces decode request primitives and select EKO metadata,
  then construct the framework value directly.
- Standard `From`/`TryFrom` is used only where a genuinely distinct product
  type must cross a boundary; source-named `to_framework_*` and
  `from_framework_*` helpers are not public API.
- The LLM facade exports `LlmConfig`, clients, and product-neutral contracts
  directly. Migration namespaces named after split crates (`core`,
  `integration`, `config`, and `providers`) are removed; `llm::types` remains
  the documented low-level wire surface.
- The unexported, deprecated `AgentRunner` source is deleted. Agent construction,
  tracing, and evaluation use the real `ReactAgentBuilder`, `RunStore`, and
  `EvalRunner` APIs directly instead of a facade that stores incomplete future
  integration fields.

The same rule applies to delivery envelopes, Subagent outcomes, task values,
and future product-neutral capabilities. If the framework type is missing a
needed operation, extend the framework instead of creating an application
mirror.

## Alternatives considered

1. Keep application DTOs and rename conversion methods. Rejected: this hides,
   but does not remove, the duplicate authority.
2. Move EKO workspace or UI policy into the framework. Rejected: those values
   are product-specific and would reduce SDK reuse.
3. Keep request parsing in every surface. Rejected: parsing and validation are
   generic framework behavior and belong beside the framework types.

## Consequences

- Framework consumers receive one typed permission/delivery/outcome model.
- Framework consumers receive one product-neutral typed execution usage value.
- EKO surface code remains responsible for transport decoding and product
  metadata, but no longer owns a framework-shaped DTO or conversion helper.
- Development data may change shape; no compatibility shim is retained.
