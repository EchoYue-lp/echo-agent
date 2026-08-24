# ADR 0001: Sender-Scoped Channel Sessions

- Status: Accepted
- Date: 2026-08-24
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

The key is private, so this changes runtime behavior without adding a second
public session API or a serialized compatibility contract. Timeout pruning
continues to reclaim identified sessions under the existing `SessionConfig`.

## Verification

Regression coverage exercises same-group multi-sender isolation, independent
mutexes, mode and HITL isolation, sender-local reset, same-sender reuse,
channel/conversation separation, timeout callback identity, invalid identity
fail-closed behavior, and QQ/Feishu ingress rejection.
