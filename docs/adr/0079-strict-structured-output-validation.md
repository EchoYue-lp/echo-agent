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
call and validates final candidates. Accepted same-turn steer input is drained
before checking a superseded text candidate. For actual model final text, the
single Output Guard check produces the authoritative answer before schema
validation and the optional Critic. For `final_answer`, ToolOutput Guard
produces each tool result; the entire batch is projected and durably settled
before schema validation, optional Critic, or a correction note. Among multiple
`final_answer` calls in one batch, the last schema- and Critic-accepted answer
wins. The run driver drains accepted steer input and fences cancellation before
selecting candidates, interrupts a waiting Critic on cancellation, and fences
again before entering final callbacks. Text-branch Critic waiting uses the same
cancel-aware fence. The first final-answer callback is the cancellation
classification safe point: a later token cancellation cannot reclassify an
already observed answer as Cancelled. Final-answer interventions retain their
existing explicit block/cancel authority after callbacks. A schema correction
consumes one retry only when no candidate in that batch passed schema validation.
Invalid JSON or a schema mismatch
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
- A schema correction note never interrupts the assistant tool-call/result
  batch in model Context or its durable transcript projection.
- The text candidate remains outside user-visible assistant transcript
  projection while the Stop hook is pending. A cancelled Stop wait cannot
  persist an answer that was never published as `FinalAnswer`, including when
  the Hook returns a continuation. The Hook's continuation retains the draft
  as input for the next iteration only after the post-Hook cancellation check.
- A final-answer intervention can still explicitly block or cancel after
  callbacks. The candidate is not written as a normal assistant transcript
  message until that intervention allows it.
- Errors and repair notes include only bounded schema paths and error type,
  never the model's raw response value.
- Provider streaming tokens and tool-result events remain provisional; they
  are not success terminals and may precede local final validation.
- Rust target-type deserialization after a successful schema-validated run is
  a caller-side type check, not a second run terminal authority.
