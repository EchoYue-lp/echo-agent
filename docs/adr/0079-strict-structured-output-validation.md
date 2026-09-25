# ADR 0079: Strict structured-output validation before terminal success

## Background

ADR 0045 DU-97 requires local JSON Schema validation and bounded repair for
strict output. The main ReAct loop has two success paths: buffered model text
and the `final_answer` tool. Checking only `execute_typed` after `execute`
returns is too late: callbacks, audit, trace, and `FinalAnswer` have already
reported success. A provider `response_format` hint does not constrain tool
arguments or guarantee a locally valid final value.

The [OpenAI Structured Outputs cookbook](https://cookbook.openai.com/examples/structured_outputs_intro)
distinguishes response-format schemas from strict function schemas and
describes refusals separately. The pinned
[`jsonschema` 0.49.9 API](https://docs.rs/jsonschema/0.49.9/jsonschema/)
documents that synchronous validator construction can block or panic in an
async runtime when external references need network retrieval.

## Options

1. Trust the provider hint. This leaves unsupported providers and
   `final_answer` tool results without a framework guarantee.
2. Validate only the typed caller result. This contradicts the run's earlier
   success observations and gives no repair opportunity.
3. Compile the caller schema locally, reject external reference retrieval,
   and validate both ReAct success paths before terminal publication. Use one
   invocation-local repair counter independent of the optional Critic.

## Decision

Use option 3. `JsonSchema { strict: true }` is compiled before the first model
call and validates every final candidate. Invalid JSON or a schema mismatch
adds a content-free correction note and consumes one bounded retry; exhaustion
returns a typed failure and finalizes the run as failed. `JsonObject` checks
JSON syntax, while `strict: false` sends a provider hint without locally
enforcing its schema. An explicit `Text` format remains unconstrained.

Local `$defs` references are allowed. External HTTP/file `$ref` targets are
rejected before the model call: schema validation must not initiate hidden
network or filesystem side effects in an Agent turn. Callers can bundle the
definitions into the schema. An explicit async resolver would require a
separate public contract for authority, cancellation, caching, and provenance.

## Consequences

- Neither text nor `final_answer` can reach final callbacks, completed trace,
  final audit, or `FinalAnswer` until the local format check succeeds.
- Repair attempts consume the existing iteration budget and a separate
  schema-retry counter. The Critic's fail-open exhaustion does not apply.
- Errors and repair notes include only bounded schema paths and error type,
  never the model's raw response value.
- Provider streaming tokens and tool-result events remain provisional; they
  are not success terminals and may precede local final validation.
- Rust target-type deserialization after a successful schema-validated run is
  a caller-side type check, not a second run terminal authority.
