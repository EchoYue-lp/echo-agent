# ADR 0081: Token-budgeted recent conversation tail

## Status

accepted

## Context and alternatives

A message-count split can put the active user request into the summarized
span when a single turn produces many tool results. Keeping all user messages
forever is unbounded; duplicating the EKO goal store in the framework would
create a second authority. Existing ContextManager, compressors, projection,
transcript and runtime-checkpoint boundaries already provide the owners.

The selected reference patterns are Codex's bounded retained user material
and reconstructed initial context, and Pi's token-budgeted recent tail with
tool-call/result boundaries and split-turn handling:

- https://github.com/openai/codex/blob/53eaa297e595fc98df0f33d4c63686a7014d7c9a/codex-rs/core/src/compact.rs
- https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/compaction.md

These patterns support bounded raw material plus summary and reconstructed
authority. They do not require a second runtime or permanent raw user history.

## Decision

Extend SummaryCompressor, IncrementalSummaryCompressor and
SlidingWindowCompressor with `with_recent_token_budget`. Zero retains the
legacy constructor's message cap. One internal pure selector partitions old
and retained messages; it owns no state. Token mode retains older turns whole.
A large active turn may retain its exact request plus recent atomic execution
blocks. Its request may exceed the soft recent allowance, but cannot exceed
the hard input allowance. Such input fails explicitly, without text truncation.

`FrameworkConfig.agent.compress_window = 0` opts into a recent allowance of
25% of the live Agent window, capped at 20K tokens. Positive values retain
the legacy cap. Framework defaults remain compatible; applications choose
their default. Hybrid's configured stages share this allowance. Adaptive
levels retain their existing separate policy.

The automatic compact boundary reads the latest real request before horizon
folding and passes it as summary focus. Runtime notes and projections are not
user requests. Protected projections, canonical system context, transcript,
checkpoint and long-term memory keep their existing owners. A noncontiguous
selection does not pretend that its covered messages form one index range.

Manual focus and provider cancellation flow through one ContextManager
operation and one ReactAgent hook/trace lifecycle. Protected payloads are
subtracted before compression and the reconstructed result is checked against
the input allowance. Cancellation before transform commit preserves context;
product journal settlement after a successful transform stays application-owned.

## Impact and validation

Incremental calculation derives its previous checkpoint from accepted input
messages. Its `current_summary`/`current_structured_summary` cache is an
observation only. `ContextCompressor::context_committed` has a default no-op;
ContextManager invokes it after final budget/verification/promotion succeeds,
and on explicit restore/clear. Hybrid forwards the notification. A rejected or
cancelled candidate cannot publish this cache. Standalone compressor users
notify acceptance explicitly rather than treating calculation as a commit.

Runtime Hook notes and rich tool attachments use the existing runtime-context
marker, including multimodal text. Tool batches close over their call IDs across
these notes; no User-role image produced by a tool becomes the active request.
Cancellation is checked before calculation and before acceptance. A zero remaining
allowance fails before fallback can reinterpret zero as a legacy policy.

Existing Rust constructors and method signatures remain available. The optional
methods are reusable Rust API, with no new protocol, store or lifecycle owner.
EKO's goal/recovery state remains in echo-agent-cli. Rollback removes the new
opt-in config and restores the old constructors; transcript history is retained.

Regression coverage includes an exact Chinese request before a tool batch,
whole recent turns, a split active turn, oversized input, repeated summary and
incremental passes, protected budget accounting and pre-transform cancellation.
