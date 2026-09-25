# ADR 0076: Guard Direction and Boundary Authority

## Status

Accepted on the Issue #57 repair branch

## Context

`GuardDirection` exposes four semantic boundaries: user input, final answer,
tool input, and tool output. The implementation previously used only the first
and the generic output direction. That made tool-specific policies unreachable,
and a guard backend error was reduced to `Warn`, allowing an unknown policy
decision to continue.

The execution pipeline also runs permission approval before tool invocation.
Changing tool arguments after the approval receipt is issued would authorize a
different payload from the one executed.

## Decision

Keep all four directions as live contracts with one owner per boundary:

- `Input` guards run before user content enters the model context.
- `ToolInput` guards run after all hook/permission rewrites and immediately
  before invocation. They are block-only; `Transform` is converted to a block
  so the receipt remains bound to the executed input.
- `ToolOutput` guards run on every buffered and streaming tool result, before
  truncation, trace, audit, and callback settlement. They may block or transform
  the result projection. With a configured GuardManager, streamed Output and
  Progress events are suppressed; only the guarded, budgeted terminal
  `ToolResult` is delivered. Per-channel live chunk identity cannot be
  preserved across a transform, so no synthetic stdout/stderr event is
  invented. With no GuardManager, live timing and channels are unchanged.
  Output and error text are independently checked when both exist. Structured
  `data`, non-empty metadata, content-bearing result kind, and MIME type are
  also checked as canonical text. A `Pass` keeps each checked structured value;
  any replacement retires parallel renderable projections that cannot be
  losslessly reconstructed, leaving only checked text. Opaque image URL/model
  content and pre-guard artifact references are suppressed whenever a Guard is
  configured, even after a text `Pass`: a text guard cannot prove that their
  referenced pixels or unseen bytes are safe. Typed failure and confirmed
  effect facts remain intact.
- `Output` guards run at the final-answer text terminal boundary, before
  callbacks, transcript persistence, trace finalization, or `Token`/`FinalAnswer`
  delivery. If the provider fails after partial content, the partial `Token`
  is also guarded before delivery. A `final_answer` tool result is already
  governed by `ToolOutput`; it is not checked a second time after being copied
  into the transcript. Reasoning `ThinkStart`/`ThinkEnd` tokens are not final
  answer content and remain a separate observation contract.

`GuardManager::check_all` propagates a guard error. Each boundary converts that
unknown decision to a fail-closed terminal outcome; errors are never downgraded
to warnings. For an error-only tool result, the guarded diagnostic replaces
`ToolResult.error` before Trace, Audit, callback, caller, and transcript
projection. `ToolFailure` free-text `postcondition` and `idempotency_key` are
checked separately. A changed postcondition uses guarded text; a changed
idempotency key is removed rather than forged. Removing the key prevents it
from authorizing automatic retry for attempts with possible or confirmed side
effects; side-effect-free attempts remain retryable when category and recovery
allow it.
After a PostToolUse block, the guarded `ToolResult.error` also replaces the
raw hook `block_reason` consumed by caller error and skill telemetry. Typed
category/recovery/side-effect fields and confirmed effects are not rewritten.
Confirmed `ToolEffect` paths remain visible to caller and Trace under ADR 0074
for recovery and diagnostic correlation; this Guard contract does not promise
generic redaction of typed effect facts.
The `PostToolUse` and `PostToolUseFailure` hooks are trusted user-installed
pre-presentation hooks and still run before ToolOutput guard under the existing
#102 pipeline order; they can inspect the raw result. This contract does not
claim to sanitize input to those hooks.

## Alternatives considered

1. Remove `ToolInput`/`ToolOutput` and rename everything to `Input`/`Output`.
   This hides a real tool effect boundary and prevents consumers from applying
   different policy to arguments and returned effects.
2. Allow ToolInput transforms after approval. This breaks the invocation-scoped
   approval receipt and can execute a payload the user did not approve.
3. Treat guard errors as warnings. This is fail-open at a safety boundary and
   makes the public error contract unreachable.
4. Run `Output` again on the `final_answer` tool result. This would duplicate
   policy evaluation and could leave an untransformed transcript projection.
5. Guard each stream chunk independently. A secret or policy phrase can span
   chunk boundaries, so per-chunk approval cannot prove aggregate safety.
6. Buffer and replay raw chunks after a terminal Pass. This would require
   unbounded raw retention or a second artifact authority, and transformed
   output cannot be mapped back to the original stdout/stderr chunks.
7. Synthesize one combined `Stdout` event from the guarded result. This would
   misrepresent stderr or mixed-channel output and duplicate the terminal
   `ToolResult` instead of preserving the typed stream contract.
8. Keep structured or rich content when the textual guard changes output.
   This would give caller and model two conflicting presentations of the same
   result and let structured MCP data or image payload bypass a text block.

## Consequences

Tool-specific guards are now production-reachable for both streaming and
non-streaming calls. A failing guard may reject an invocation even when the
underlying tool is otherwise available, which is the required fail-closed
behavior. Existing `Output` guards continue to protect text final answers.
Guarded tool calls trade rich image delivery and pre-guard artifact retrieval
for this fail-closed boundary; tools with several distinct renderable fields
also pay for their separate Guard checks.

## Verification

Focused regressions cover direction capture, block-only ToolInput behavior,
streaming and non-streaming pre-invocation blocking, guarded streaming
Block/Transform without raw chunk leakage, error-only Block/Transform/Err
projection across caller/Trace/Audit/callback/transcript, final-answer Output
reachability, blocked/changed `Token` delivery, provider-failure partial
content, and propagation of guard errors. Rich tool output tests check that
blocked data and image content cannot re-enter the model context, and failures
with non-empty output check an independent error diagnostic. A text `Pass`
with sensitive data/metadata/kind/MIME still blocks those independent fields;
without a Guard, rich content remains available. Hook reasons quoting raw
output and partial-failure free-text recovery hints are covered separately.
