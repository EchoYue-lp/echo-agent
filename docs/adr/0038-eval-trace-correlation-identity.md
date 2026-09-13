# ADR 0038: Eval resolves trace identity through invocation correlation

- Date: 2026-09-14
- Owners: `eval/runner`, `trace`, `agent/react`

## Status

Accepted.

## Context

`EvalRunner` currently calls `Agent::current_run_id()` after a Turn settles,
stores that value in `EvalResult.run_id`, and uses it as the key for
`RunStore::load`. That getter represents an external product or business run.
ReactAgent independently creates a unique trace invocation ID in
`start_scoped_trace_run`; its `Run` stores the product run as
`parent_run_id` and carries turn and execution correlation separately.

Consequently, Eval can expose a product ID as though it were a trace ID and
fail to load the trace that was just finalized. Trace-based criteria,
constraints, and metrics then consume no Run even though one exists.

OpenTelemetry keeps Trace ID in immutable Span Context and propagates context
instead of conflating it with an application request. OpenAI Agents SDK also
separates a generated or supplied `trace_id` from a `group_id` used to
correlate multiple traces. OpenAI Codex keeps thread, turn, runtime operation,
and telemetry trace identities typed and distinct.

- <https://opentelemetry.io/docs/concepts/signals/traces/>
- <https://openai.github.io/openai-agents-python/tracing/>
- <https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/protocol/v2/turn.rs>
- <https://github.com/openai/codex/blob/main/codex-rs/otel/src/trace_context.rs>

## Options Considered

1. Keep using `Agent::current_run_id()`. This is the confirmed authority
   conflict and is not invocation-scoped trace state.
2. Expose ReactAgent's mutable `current_trace_run_id`. Eval accepts arbitrary
   Agents, and a shared getter cannot prove that the value belongs to the Turn
   that just settled.
3. Add `trace_run_id` to `TurnReceipt`, `AgentEvent`, or the Agent trait. Trace
   is optional observation rather than a terminal control fact, so this would
   expand every Agent and SDK contract without necessity.
4. Scan traces and pick the newest. Ordering is not identity and becomes
   incorrect under concurrent or nested execution.
5. Give each Eval invocation one unique run/turn/execution correlation through
   existing value-scoped context, then resolve the producer-owned trace from
   the existing RunStore correlation fields.

## Decision

Choose option 5.

- Eval's existing unique invocation value becomes a formal correlation. It is
  used with `EventIdentity::for_run` and copied into
  `ExternalRunContext.run_id`, `turn_id`, and `execution_id`.
- `AgentTurnDriver` and `TurnReceipt` retain their existing public shape and
  terminal authority.
- ReactAgent remains the only owner that allocates the real trace
  `Run.run_id`. Its existing trace start path stores the Eval correlation as
  `parent_run_id`, `turn_id`, and `execution_id`.
- Only after a TurnReceipt exists may Eval call
  `RunStore::list_by_parent_run(correlation)`. Candidates must also match the
  exact turn and execution IDs.
- Exactly one matching summary is loaded by its real trace ID. Only a
  successfully loaded `Run` can populate `EvalResult.run_id` and feed trace
  criteria, constraints, and metrics.
- No matching trace is legal because Agent implementations are not required to
  produce trace data. Multiple exact matches, list failure, load failure, or a
  dangling summary are authority failures: Eval records a violation and fails
  rather than selecting by order.
- An unsettled timeout performs no trace list or load operation, preserving ADR
  0037.

## Identity Authority

| Identity | Owner | Purpose |
| --- | --- | --- |
| Eval correlation | EvalRunner | Value-scoped parent, turn, and execution linkage for one case invocation |
| Product/business run | Embedding application | External task or workflow correlation outside Eval |
| Trace run ID | Trace producer and RunStore | Primary key of one concrete observation Run |

`EvalResult.run_id` means the third row only. It never contains the Eval
correlation or a product run ID.

## Failure Semantics

- Zero candidates: continue with `run_id=None`; output-only criteria remain
  usable, while trace-required criteria retain their existing missing-trace
  behavior.
- A candidate with another turn or execution ID is ignored.
- More than one exact candidate: fail with an ambiguity violation.
- RunStore list/load error or an exact summary whose Run cannot be loaded: fail
  with a trace lookup violation and do not set `run_id`.
- Settled Completed, Failed, Cancelled, and post-deadline receipts may load a
  diagnostic trace. Loading a trace never changes their terminal outcome.
- Grace-expired timeout does not query RunStore.

## Consequences

- EvalResult, criteria, constraints, and metrics consume one verified Run.
- Legacy product run state cannot leak into a value-scoped Eval result.
- Agents that do not trace remain compatible.
- Existing RunStore implementations and external consumers need no schema or
  trait change.
- Correlation lookup may list a product-run group, but only exact turn and
  execution identity can select a candidate.
- Public Rust and language SDK identities do not change.

## Compatibility And Rollback

The public shapes of EvalResult, TurnReceipt, AgentEvent, Agent,
ExternalRunContext, RunStore, Run, and SDK contracts remain unchanged.
`EvalResult.run_id` already claims to reference a trace Run; the change makes
that claim true instead of returning a product ID.

Rollback must restore the correlation setup and trace lookup together. Falling
back to `Agent::current_run_id()`, exposing ReactAgent mutable state, or choosing
the newest trace is not an acceptable partial rollback.

## Verification

A real ReactAgent test installs a legacy product run ID, uses the same
InMemoryRunStore as Eval, and emits provider usage. The old implementation must
fail to return/load the real trace; the repaired implementation must return the
actual `run_<uuid>`, preserve the legacy product value, and populate metrics
from that Run.

Additional deterministic stores cover no candidate, another child, duplicate
exact candidates, list/load failure, and dangling summary. Existing unsettled
timeout tests count both list and load operations and require zero. Eval,
ReactAgent, TurnDriver, bilingual documentation, SDK zero-diff, semantic,
Issue, and independent review gates remain required.
