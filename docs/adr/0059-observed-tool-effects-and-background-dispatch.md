# ADR 0059: Record Confirmed Tool Effects at Their Owning Boundary

- Date: 2026-09-18
- Owners: `echo-core::tools`, `src::agent::react`, `src::eval`

## Status

Accepted

## Context

`RunEvent` declared file edits, test runs, and Subagent runs, but the default
tool and dispatch paths did not reliably produce them. A successful tool call
does not establish that a file changed; a shell exit code does not disclose
the number of failed tests; and `agent_tool(background=true)` returns before
its Subagent settles. A shared tool can also serve overlapping Agent turns, so
reading a mutable parent trace ID at background completion could attach a fact
to the wrong run.

The framework owns generic tool, eval, and Subagent execution facts. EKO owns
its UI and product task projections, which do not belong in this contract.

## Industry References

- The official [Codex SDK item types](https://github.com/openai/codex/blob/main/sdk/typescript/src/items.ts)
  distinguish a command's process exit from a file-change item, while
  [events](https://github.com/openai/codex/blob/main/sdk/typescript/src/events.ts)
  distinguish item completion from enclosing turn completion.
- [OpenTelemetry recording-error guidance](https://opentelemetry.io/docs/specs/semconv/general/recording-errors/)
  attaches failure to the operation observed, rather than promoting a handled
  child failure to an enclosing operation failure.

Both point toward typed facts emitted at the producer's terminal boundary,
with correlation to the invocation that caused them.

## Decision

1. Successful `read_file` emits `ToolEffect::FileRead` using its resolved path;
   failures and invalid/undeliverable ranges emit none. Create, delete, write,
   append, update, move, and patch tools emit
   `ToolEffect::FileEdit` only after confirmed mutation, never for a dry run or
   proposed patch. A move records both source and destination; incomplete patch
   rollback inspects only attempted paths. ExecuteStage projects typed effects
   before post-use presentation policy, without inferring effects from names,
   output text, paths, or generic success.
2. `TestRun.failure_count` is optional. A structured test report may provide
   an exact `Some(count)`; an explicit `EvalRunner::TestPass` or SWE-bench
   command that actually completes supplies a test-command result with
   `None`. Shell status 126/127, a signal, or a process launch failure does
   not prove the configured test runner executed, so no TestRun is emitted.
   The evaluator appends to its exact correlated Agent trace and reports
   rejected trace writes in its result.
3. Synchronous `agent_tool` attaches one Subagent terminal effect to its tool
   result, including failed/cancelled outcomes. Background dispatch attaches
   no effect to its launch acknowledgement; the tool retains the returned
   handle and observes exactly one terminal result. A `ToolContext` effect
   sink captures the parent trace and tool `call_id` at invocation and reports
   diagnostic append failures through the existing observer. It never reads
   the next turn's mutable parent state. Typed failed and timed-out Subagent
   results publish `DispatchFailed`, never `DispatchCompleted`; a returned
   non-terminal status is normalized to failure.
4. `RunEvent::SubagentRun.call_id` and both TestRun counts are optional when
   decoding old serialized records; newly observed tool dispatches carry the
   call ID. Subagent execution ID remains on its existing lifecycle event and
   launch result, linked to the trace by call ID.
5. On an outer tool timeout/cancel, keep every completed tool's actual result
   and synthesize a failed terminal only for calls that have not completed.
   Publish all assistant call results in call order, including future waves,
   writing context before observer delivery. For a call that never reached
   ExecuteStage, record the missing ToolCall and `ToolExecutionSkipped` before
   its synthetic failed ToolResult/ToolError; do not duplicate an executed
   call. Eval, trace error analysis, improvement critique, and evolution review
   exclude skipped IDs from executed-tool failures/evidence. Trajectory export
   still preserves the admitted request/result pair. An interrupted effect
   remains `possible`, never confirmed.
6. Replay and Eval file-change constraints consume distinct confirmed FileEdit
   paths, not ToolCall arguments or read attempts. Successful typed FileRead
   results establish read-before-edit using the actual resolved path; an
   attempted read alone does not.
7. A synthetic terminal for an invocation that never reached ExecuteStage is
   paired with `ToolExecutionSkipped`. Eval and improvement critique exclude
   the marked call from executed-tool usage without weakening ToolCall/ToolResult
   trajectory pairing; trajectory export counts it as an admitted request. An
   invocation-local call-ID set, not diagnostic-store visibility,
   is authoritative for whether ExecuteStage started. The runtime invokes
   `on_tool_interrupted_with_id` exactly once for
   that synthetic terminal. Its full admitted input lets AuditCallback create
   one correlated failed audit record whether or not normal START ran; it
   never fabricates a START event.
8. If ExecuteStage has started but the tool returns a typed cancellation or
   timeout error before returning a `ToolResult`, CallbackEnd also uses
   `on_tool_interrupted_with_id` once with the admitted input and final
   settled reason. The typed failure retains the underlying cancellation or
   timeout category if a post-use policy rewrites the terminal reason. This
   preserves an unknown external effect without marking the call
   skipped. A tool that returns an actual failed `ToolResult` continues through
   ordinary `on_tool_error_with_id`; a failure category alone is not proof of
   interruption.

## Alternatives Rejected

- Guessing file changes from `write_file` success or test counts from shell
  output/exit codes fabricates facts for partial writes, dry runs, and runners
  without reports.
- Treating a background launch as a completed Subagent produces false success
  and misses later failure or cancellation.
- Storing the current parent trace ID on the shared `AgentDispatchTool` permits
  concurrent turns to overwrite one another's identity.
- Requiring a universal test-output parser would tie the generic framework to
  one runner's report format. A future adapter can provide exact counts.

## Consequences

Consumers can distinguish known from unknown test counts without coercing an
unknown to zero. Effects may arrive after a parent turn's terminal event when
the tool deliberately started background work; the append-only RunStore keeps
those later facts on the original trace. Cancellation and dispatch errors are
Subagent outcomes, not parent run-level errors. A process abort still cannot
guarantee in-process background observation; durable recovery would require a
separate owner and contract. [ADR 0053](0053-trace-audit-persistence-visibility.md)
remains the authority for best-effort diagnostic delivery and failure reporting.
