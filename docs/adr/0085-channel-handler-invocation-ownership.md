# ADR 0085: Original Channel Handler Invocation Ownership

- Date: 2026-10-13

## Status

approved

Implementation is authorized. This follow-up starts at the reviewed owned-Channel
snapshot `4eb005c`; it does not modify that prior milestone's frozen source or
reuse its checks as proof of the new API. Fresh validation and main acceptance
remain pending.

## Context

ChannelHandle owns the registered plugin and its handler cleanup responsibility,
but provides no forward entry into that handler. Merely cloning a handler Arc
would allow calls during failed start/rollback, after stop, or on a replacement
handler, and would make slow streams outlive awaited shutdown.

## Alternatives

1. Expose bare handler clones and ask callers to coordinate shutdown. This loses
   the original entry's admission and actual settlement responsibility.
2. Introduce another handler registry or executor. This duplicates an existing
   lifecycle authority and pollutes the framework with consumer address policy.
3. Extend the original entry with success-only generation admission and invocation
   receipts, using the existing cancellation and owned-stream patterns.

## Decision

Choose option 3. ChannelHandle exposes `handle_message`,
`handle_message_stream`, and `reply_message`. Each accepts the original message
and an invocation-specific CancellationToken. Single calls are driven by the
original generation's tracked task; streams return `ChannelMessageStream`, an
owned `BoxStream<'static, Result<OutboundMessage>>` plus its exact admission's
child cancellation token. The bundle implements Stream and offers into_parts;
adapters watch this same token while blocked on their own bounded sends, avoiding
old buffered delivery or shutdown deadlock without querying another generation.
This supporting Rust/Host representation does not expand accepted SDK identities.
Their driver captures the original
handler Arc and receipt and creates the trait's borrowed stream inside that
ownership scope. No transmute or leaked lock guard is permitted.

The original entry remains the only handler and cleanup authority. A small
generation admission/active-receipt record extends that entry, following
SessionGeneration/SessionStreamReceipt's synchronous RAII accounting. It has no
second handler map, business scheduler, durable state, or execution policy.
Start publishes cleanup responsibility before its first await, and opens
invocation admission only after successful start. Failed/cancelled start leaves
cleanup debt closed to new messages.

Each invocation has a generation child token. Cancelling one caller or dropping
its waiter cancels only that child. Stop retires generation admission and signals
all children before acquiring the entry write lock. After transport stop and
actual invocation receipt settlement, it closes the original handler. Failed or
cancelled close retains the same handler/generation for retry; restart creates a
new generation only after prior cleanup. Captured calls cannot switch generations.

MessageHandler's cancellation-aware single-call defaults preserve legacy method
semantics. Legacy calls may stop by dropping their local future; this proves no
external Run terminal. Implementations declaring `settles_on_cancel` must pass
the exact token to their underlying driver and continue until its real result or
stream termination. AgentChannelHandler's single-call override reuses its
existing AgentTurnDriver and terminal receipt, without a second Run runtime.
Uncooperative driven handlers retain shutdown debt instead of reporting success.

Metadata reads remain short and do not await handler locks. Recursively awaiting
mutation/close of the same entry from its callback is not supported. SDK wire,
resource address/generation, language catalogs, and EKO UI policy remain outside
the framework. New Rust methods do not automatically become cross-language
accepted identities.

Inactive/starting/failed-start admission and a mismatched message channel use
the existing ChannelError::InvalidConfig. A captured invocation whose original
entry generation was replaced returns typed ChannelError::StaleInvocation; it
does not borrow StaleDelivery's distinct session-output meaning. Adapters map
these to existing invalid-value/closed-handle categories without matching error
text. This Rust variant is Host-only inventory by default, not a new accepted SDK
wire error code. Missing Tokio runtime fails before admission/spawn, avoiding a
tokio::spawn panic.

## Industry evidence

[Tokio shared state](https://tokio.rs/tokio/tutorial/shared-state) distinguishes
short registry locks from operation ownership. [Tokio RwLock](https://docs.rs/tokio/latest/tokio/sync/struct.RwLock.html)
documents a held-read/queued-write/subsequent-read deadlock. Closing admission and
signalling cancellation before waiting on lifecycle locks avoids that dependency.
[Tokio shutdown](https://tokio.rs/tokio/topics/shutdown) and
[MCP cancellation](https://modelcontextprotocol.io/specification/2025-06-18/basic/utilities/cancellation)
separate cancellation requests from actual settlement. The framework therefore
waits its original invocation receipts; cancellation notifications are not proof
of terminal execution or operating-system process cleanup.

## Consequences and verification

Regression coverage includes original handler identity, single/stream/reply
results, failed and interrupted starts, message channel mismatch, stale captured
calls after restart, child cancellation isolation, slow/full/unpolled streams,
setup failure, dropped waiters, actual driven settlement, close failure/retry and
metadata reentry. Bilingual framework documentation and the executable module
doctest demonstrate the public entry. SDK consumer delivery is separately tested
in its repository and pins a published framework successor.

Framework focused tests, workspace gates and the public-API feature matrix run
after the dependency repair/main synchronization. Current source preparation
does not claim they have passed. Website API descriptions and independent
Channel examples are checked at delivery; existing borrowed APIs stay supported.
