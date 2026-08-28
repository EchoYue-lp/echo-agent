# ADR 0010: Canonical Turn Receipt Accounting

- Status: Accepted
- Date: 2026-08-28
- Owners: `echo-orchestration::runtime`

## Context

`AgentTurnDriver` already owned envelope sequencing, typed terminal mapping,
provider usage totals, the final answer, and elapsed time. A downstream product
adapter still folded the same stream a second time to recover token totals,
elapsed time, context-compaction count, final answer, and final message identity.
That left two summaries derived from the same invocation and allowed sink or
application behavior to make them diverge.

This decision concerns only generic invocation facts. Durable runtime history
and recovery checkpoints already belong to the framework `RuntimeStateStore`;
the user-visible transcript remains a `ConversationStore` projection. Product
workspace identity, retention pins, Task policy, and webhook delivery are not
framework turn facts.

## Industry evidence

- OpenAI Codex exposes `TurnCompletedNotification` as a complete turn object
  with stable turn identity, status, and duration. Token usage is an explicit
  notification keyed by `threadId` and `turnId`, and context compaction is an
  explicit notification rather than a UI inference:
  <https://github.com/openai/codex/tree/main/codex-rs/app-server-protocol/schema/json/v2>
- Claude Agent SDK streams progress messages but reports the final result in a
  `ResultMessage`; context compaction is a distinct compact-boundary message:
  <https://platform.claude.com/docs/en/agent-sdk/streaming-output>

Both systems keep progress projection separate from a bounded, authoritative
completion result.

## Decision

1. `TurnReceipt` is the single framework summary for one driven invocation.
   It includes typed terminal, final answer, final message identity, reported
   prompt/completion tokens, usage call count, explicit compaction count, last
   event sequence, and elapsed time.
2. `AgentTurnDriver` records those fields before handing each envelope to the
   sink. If delivery fails, terminal-only result fields are cleared with the
   failed receipt rather than being retained as a successful completion.
3. Product adapters consume the receipt for generic facts. Their event
   observers retain only product behavior such as tool webhooks, persistence,
   rendering, and product-specific policy.
4. Runtime checkpoint and transcript authorities do not move. No new history
   store, reducer, or checkpoint is introduced.

## Alternatives rejected

- Keep a product-side summary reducer: duplicates the framework driver and can
  diverge on sink failure, cancellation, missing terminal, or future events.
- Move product journals and webhooks into the framework: those depend on EKO
  workspace and surface policy and are not reusable turn mechanics.
- Infer completion from EOF or the final rendered item: EOF is not a typed
  terminal and renderer acceptance is not Agent completion.

## Consequences

Framework consumers receive one bounded completion receipt without replaying
the stream. EKO keeps its durable product journal and webhook adapter, but no
longer recalculates generic turn accounting. New generic receipt fields are a
public API addition: ordinary receipt readers remain source-compatible, while
downstream code that directly constructs a `TurnReceipt` literal must provide
the new fields.

## Verification

Framework tests cover multiple usage events, explicit compaction boundaries,
final message identity, sink failure, cancellation, missing terminal, and
sequence continuation. The application adapter test verifies that its product
outcome uses the framework receipt while event observation remains limited to
product projection.
