# ADR 0001: Sender-Scoped Channel Sessions

- Status: Accepted
- Date: 2026-08-25
- Owners: `echo-integration/channels`

## Context

`SessionHandler` originally keyed sessions by `(channel_id, conversation_id)`.
That made every participant in a group conversation share one handler, async
mutex, Agent history, mode, human-in-the-loop state, timeout lifecycle, and
reset command. This contradicted the documented per-user session model and let
one participant reset or influence another participant's Agent state.

The public `InboundMessage` type also permits callers to construct malformed
identities. Built-in QQ and Feishu adapters previously replaced a missing
sender with the shared string `unknown`, which reproduced the collision even
after adding a sender coordinate.

## Options Considered

1. Keep one session per conversation. This preserves group-wide context but
   violates the framework's per-user contract and cannot isolate mode, HITL,
   locking, timeout, or reset state.
2. Add sender-specific maps in each product or transport. This duplicates
   session authority outside `SessionHandler` and lets adapters drift.
3. Use a message-scoped anonymous session when the sender is missing. This
   avoids cross-message state sharing, but malformed traffic would retain one
   Agent per message until timeout and grow memory linearly.
4. Use one typed framework key for identified senders and reject malformed
   identity coordinates. This keeps one authority and bounds retained sessions
   to identified channel participants.
5. Derive runtime identity only from the stable sender key. This cannot
   distinguish a replacement handler from its predecessor, so a persistent
   runtime store may silently restore model messages after timeout or reset.
6. Give every concrete handler a framework-owned opaque incarnation while
   leaving the stable sender key unchanged. This separates ephemeral runtime
   continuity from product conversation history without imposing an EKO data
   model on the framework.

## Decision

`SessionHandler` uses one private typed key containing `channel_id`,
`conversation_id`, and `sender_id`.

- The same sender in the same channel conversation reuses one handler.
- Different senders, conversations, or channels use different handlers and
  mutexes.
- Reset and timeout replacement affect only the exact sender-scoped key.
- `active_sessions()` counts retained sender-scoped sessions, not conversations.
- All three identity coordinates must be non-empty, must not contain
  surrounding whitespace, and `sender_id` must not be the sentinel `unknown`.
- Invalid identity returns the existing typed `ChannelError` before a session
  or Agent is created.
- QQ and Feishu ingress validate the same contract and do not forward malformed
  messages to a handler. Feishu emits `open_id:{value}` or, when `open_id` is
  absent, `user_id:{value}` so the two identity namespaces cannot collide.
- Session-end callbacks report the same validated identity used by the key.
- Every factory call receives a `ChannelSessionInstance`: its three validated
  coordinates are stable and its opaque incarnation is unique to that concrete
  handler lifetime.
- Framework timeout/reset replacement creates a new incarnation. Applications
  that own a richer reset barrier can call `ChannelSessionInstance::rotate()`
  after settlement; all clones and the eventual end callback share that one
  authority.
- `SessionEndInfo.incarnation_id` always reports the incarnation that actually
  ended, including an application rotation, so exact runtime cleanup does not
  depend on reconstructing identities from strings.
- `AgentInvocationContext.runtime_state_id` separates ReAct checkpoint identity
  from the stable product conversation carried by `ExternalRunContext`.
  `transcript_generation_id` enables typed append projection for that runtime
  incarnation.
- Canonical transcript records carry an internal `(generation, ordinal)` in
  their existing projection metadata. `AgentCheckpoint` persists the committed
  ordinal/digest cursor in its existing payload, so duplicate content, pool
  eviction, crash cuts, and post-compaction suffixes do not require content
  boundary guesses.

No anonymous per-message fallback is retained, and anonymous messages do not
reuse state across deliveries.

## Consequences

Group participants now have independent Agent history, mode, HITL state,
locking, reset, and timeout lifecycles. Direct-message routing and state reuse
remain the same for valid transport identities, but handlers and session-end
callbacks now observe Feishu sender identities in the canonical
`open_id:{value}` or `user_id:{value}` form instead of a raw provider value. A
malformed transport event is rejected instead of receiving an Agent response;
transport owners must provide a stable sender identity before entering the
framework.

The key remains private and no parallel session store or serialized contract is
introduced. `ChannelSessionInstance` is a lifecycle capability supplied by the
existing `SessionFactory`, not a second lookup authority. Consumers may include
its incarnation in model/checkpoint keys while retaining the three stable
coordinates for journals, TaskRuns, and UI routing. Timeout pruning continues
to reclaim identified sessions under the existing `SessionConfig`.

An incarnation rotation is a model-context boundary, not product-history or
disk deletion. A new runtime key cannot load the predecessor checkpoint, while
the stable `ConversationStore` transcript remains append-only and queryable.
After old foreground/resource settlement, callers use the durable scope lineage
from [ADR 0006](0006-runtime-state-scope-lineage.md) to delete the exact retired
runtime checkpoint. Product deletion clears every indexed incarnation and then
the stable transcript.

This decision changes the public `SessionFactory` callback shape and adds
`ChannelSessionInstance`, `ChannelSessionRotation`,
`AgentInvocationContext.runtime_state_id`, and
`AgentInvocationContext.transcript_generation_id`. Callers that do not split
identities keep the prior behavior through `None` defaults.

## Verification

Regression coverage exercises same-group multi-sender isolation, independent
mutexes, mode and HITL isolation, sender-local reset, same-sender reuse,
channel/conversation separation, timeout callback identity, invalid identity
fail-closed behavior, and QQ/Feishu ingress rejection.
Incarnation coverage additionally exercises same-handler reuse, application
rotation, timeout callback accuracy after rotation, and fresh replacement IDs.
Persistence coverage exercises same-incarnation checkpoint recovery, rotated
checkpoint isolation, identical-tail transcript append, repeated safe-point
idempotency, crash-cut catch-up, corrupt cursor rejection, and post-compaction
ordinal continuation.
