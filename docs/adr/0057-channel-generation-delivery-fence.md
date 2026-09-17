# ADR 0057: Fence Channel Delivery by Session Generation

- Date: 2026-09-18
- Owners: `echo-integration/channels`

## Status

Accepted

## Context

`SessionHandler` already assigns a unique incarnation to each sender-scoped
handler and defers cleanup until admitted streams settle. A command reset,
however, installs the replacement immediately while an older stream may still
yield. `OutboundMessage` and the built-in QQ/Feishu delivery queue carry no
authority proving that a message still belongs to the current incarnation.
An old reply can therefore cross the transport boundary after the reset reply
or after output from the replacement handler.

Cancelling only the old stream is insufficient. Cancellation is cooperative,
and a chunk may already have left the stream while it is queued or being sent.
Checking only when a chunk is yielded has the same check-then-send race.

## Industry References

- Claude Code records a fix so a Stop or interrupt received around turn start
  stops that turn instead of allowing it to run to completion. Its release
  history also treats interrupted work as unfinished rather than successful
  continuation.
  [Claude Code release history](https://github.com/anthropics/claude-code/blob/68ac8bbf0245b615b41517bf8f2b2f35af1ae31d/feed.xml)
- Codex binds work to a generation-prefixed identity and rejects cells from a
  stale host generation. Its session lifetime also exposes a reset version and
  child cancellation token, keeping generation validation and cancellation as
  complementary mechanisms.
  [Codex generation fencing](https://github.com/openai/codex/blob/fcf05456bb27e6c3d5677550f54011db6a2a0817/codex-rs/code-mode/src/grpc_session/generation.rs),
  [Codex history reset lifetime](https://github.com/openai/codex/blob/fcf05456bb27e6c3d5677550f54011db6a2a0817/codex-rs/core/src/session/mod.rs)

The common pattern is that interruption retires the old lifetime, while a
generation check rejects stale work at the side-effect boundary.

## Options Considered

1. Let old streams drain and delay only cleanup. This preserves existing work
   but lets stale replies contaminate the replacement conversation.
2. Cancel old streams without a delivery fence. This reduces stale output but
   leaves a race for chunks already returned or queued.
3. Add an incarnation string to the wire payload and require each application
   to compare it. This exports framework lifecycle policy and duplicates the
   authority in every transport consumer.
4. Reuse the existing `SessionGeneration` as owner of an opaque delivery fence.
   Each transport admission takes a short-lived lease; reset retires the fence,
   cancels the old stream, and waits only for already admitted leases before it
   installs and acknowledges the replacement.

## Decision

Choose option 4.

- `SessionGeneration` remains the single lifecycle authority. It owns one
  cancellation lifetime and one opaque delivery fence for its incarnation.
- Application `rotate()` advances the current incarnation token inside the
  same generation-level fence state. Older tokens stop accepting delivery, but
  their already active leases remain in the shared counter, so a later
  framework reset still waits for them.
- `SessionHandler` stamps output from that generation with the opaque fence.
  Custom handlers remain unfenced unless they are wrapped by `SessionHandler`.
- The common QQ/Feishu delivery helper acquires a lease before queue admission.
  A retired fence returns a typed stale-generation channel error and no network
  request is enqueued.
- A delivery lease remains active through the actual transport result. Reset
  retires the fence and waits for already admitted leases to settle before the
  replacement/reset reply becomes observable. Work that was accepted before
  reset may finish before reset acknowledgement; it cannot finish after that
  acknowledgement.
- Retirement also cancels the old stream wrapper. The wrapper stops polling and
  releases its existing stream receipt, so cleanup can settle without waiting
  for a cooperative inner stream.
- Timeout replacement remains restricted to an idle generation and uses the
  same retirement path before replacement.
- No new store, retry queue, serialized field, protocol message, or product
  conversation state is introduced. The fence is process-local Rust lifecycle
  metadata and is not a language-SDK or wire contract.

## Consequences

Reset becomes a delivery barrier rather than only a handler swap. It may wait
for an already running network send, but it does not wait indefinitely for an
old model stream. Once reset is acknowledged, no output from the retired
incarnation can be newly admitted by the built-in channel transports.

Dropping a transport future cannot retract a request already accepted by a
remote provider. The contract therefore linearizes at the local transport
admission lease: reset waits for such admitted work and only then acknowledges
the new generation. Provider-side duplicate delivery, retries, and remote
ordering remain transport concerns.

Opaque lifecycle metadata on `OutboundMessage` is private to the channel module,
so external code cannot clear or move the authority. This makes direct external
struct-literal construction incompatible; `OutboundMessage::new` remains the
supported constructor and supplies the unfenced default for custom handlers.
The typed `ChannelError::StaleDelivery` variant is the only newly public Rust
identity. Neither the private fence nor the error is serialized or exposed on
channel wire protocols or language SDKs.

## Verification

Regression coverage must park a real built-in delivery receipt, start reset,
prove reset cannot acknowledge early, settle the admitted delivery, and prove
the old stream cannot enqueue another chunk. Additional focused tests cover
typed rejection after retirement, cancellation during blocked stream setup,
application rotation with an older active lease, cancellation/drop settlement,
direct transport sends, timeout replacement, reset replies, and unchanged
unfenced custom-handler behavior.
