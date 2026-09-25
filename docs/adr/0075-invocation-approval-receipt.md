# ADR 0075: Invocation-Scoped Tool Approval Receipt

## Status

Proposed on the Issue #37 repair branch

## Context

`PermissionService` authorizes an Agent tool invocation, while `ShellTool` has
an independent `CommandPolicy` that classifies the concrete command.  Before
this repair an approved `Execute` permission was not carried to the shell
effect boundary.  A command classified as `RequiresApproval` could therefore
be rejected after the user had already approved the same call.  Conversely, a
receipt that was not bound to the rewritten arguments could authorize a
different command.

The framework must keep the generic permission decision in the framework, and
must not move EKO-specific UI or approval state into `ToolManager`.  The
receipt is a transport value for one invocation, not a second policy store.

## Industry references

Claude Code exposes permission modes and rules separately from tool execution;
the decision applies to the concrete tool call and can be scoped to a session
or one invocation.  Codex similarly separates sandbox policy from the
approval decision while reporting one canonical command invocation.  These
implementations support carrying an approval result to the final effect
boundary instead of asking the tool implementation to repeat the human loop.

## Options considered

1. Let `ShellTool` call `PermissionService` again.  This creates a reverse
   dependency and repeats the approval interaction.
2. Store approved commands in a shared registry.  This introduces stale and
   cross-invocation reuse, especially across rewrites and retries.
3. Carry an invocation-scoped receipt bound to final tool name and canonical
   effective arguments.  The permission authority issues it, the pipeline
   transports it, and the effectful tool validates it immediately before the
   effect.

## Decision

Use option 3.

- `ToolApprovalReceipt` lives in `echo-core` and records the final tool name
  plus recursively canonicalized JSON arguments.
- `PermissionService` issues the receipt only for an `Allow` decision and
  binds it to the handler's updated input when approval rewrites arguments.
- Explicit `Allow` decisions from trusted permission hooks issue the same
  receipt after all hook rewrites have been applied.
- `ToolContext` carries the receipt for one invocation.  Shared tools must not
  retain it across calls.
- `ShellTool` uses one receipt helper for foreground, background-cell, and
  streaming execution.  A `RequiresApproval` command executes only when the
  receipt matches the exact final parameters.  `Dangerous` commands remain
  denied even with a receipt; direct `ToolManager` calls without a receipt
  remain denied.
- The legacy snapshot approval method remains as a compatibility wrapper that
  exposes only rewritten input.  The Agent execution pipeline uses the receipt
  aware method.

## Consequences

Shell callers that directly invoke an effectful command must explicitly provide
an exact receipt; this is intentional fail-closed behavior.  Safe commands are
unchanged.  The public `ToolContext` and permission result carry one additional
optional field, but no application-specific state machine or persistence is
introduced.

## Verification

Focused tests cover canonical key ordering and tool-name binding, handler
rewrites, foreground Shell execution, background-cell launch, and streaming
execution.  The full workspace gate and public-feature matrix remain delivery
responsibilities for the PR merge branch.

## References

- Anthropic, [Claude Code settings and permission modes](https://docs.anthropic.com/en/docs/claude-code/settings)
- OpenAI, [Codex CLI approval and sandbox configuration](https://developers.openai.com/codex/cli/reference)
