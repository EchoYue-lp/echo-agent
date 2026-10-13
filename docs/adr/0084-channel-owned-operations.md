# ADR 0084: Own Channel operations independently of registry borrows

## Status

Accepted.

## Context

The SDK held its ChannelManager mutex while awaiting a channel health callback.
That callback requested the same manager's IDs and could not acquire the mutex.
Start, send, stop and handler close had the same registry-lock boundary. Replacing
that mutex with an RwLock alone would not remove nested waits.

This is a generic framework ownership problem. The SDK must release its external
registry guard before invoking the original plugin; it must not copy the registry,
create another proxy, or own a second rollback/close state machine.

## References and options

[Tokio shared state](https://tokio.rs/tokio/tutorial/shared-state) separates short
state access from asynchronous operations. The [Tokio RwLock documentation](https://docs.rs/tokio/latest/tokio/sync/struct.RwLock.html)
also explains how nested acquisition can deadlock. We use one asynchronous lock
per original channel, with metadata independent of that lock.

Options considered were a second SDK lookup registry, fixing only health, a new
actor, and owned framework entries. The first divides identity and cleanup
responsibility; the second leaves other callbacks blocked; an actor adds another
execution model without a need. Owned entries reuse existing plugin behavior.

## Decision

ChannelManager keeps one map of ChannelHandle wrappers. Each wrapper references
one entry containing the original plugin, its handler cleanup responsibility and
the confirmed transport-stop epoch. The old get/get_mut accessors delegate to
this same entry, and remain available while owned handles exist. There is no
separate handler map or stopped-transports set.

get_owned, start_all_owned, stop_owned and stop_all_owned produce access or futures
that do not borrow the manager. Batch construction snapshots registered entries
synchronously. An unpolled operation does not invoke a factory or plugin. A later
registration belongs to the next batch. Results retain channel identity and one
failure does not suppress the remaining channels.

Lifecycle mutation uses the entry write lock. Health and send use its read lock.
Start stores the original handler before awaiting the plugin; failure or future
cancellation cannot erase responsibility. Stop records successful transport stop
before awaiting handler close, so close retry does not stop that transport again.
A mutable borrowed accessor advances a saturating epoch: an in-flight stop cannot
overwrite a newer invalidation, and saturation conservatively calls transport
stop. Successful shutdown does not unregister the channel. Manager or final-entry
drop can report remaining responsibility but cannot perform awaited shutdown.

## Metadata compatibility boundary

ID, label and capabilities are registration-time declarations and remain fixed
for that registration. Queries never await a plugin lock. QQ and Feishu use
constant/static capabilities; the SDK proxy uses immutable descriptor fields;
repository consumers have no live-changing metadata dependency.

The original trait could theoretically expose different labels/capabilities after
start/stop. Managed access now deliberately returns the registration declaration,
including through get/get_mut. Third-party plugins must use a new registration
lifecycle for a changed declaration. We do not promise synchronous references to
live data behind an asynchronous lock, use unsafe references or blocking locks,
or make get_mut disappear while an owned handle exists. Asynchronous calls still
reach the original plugin instance.

## Consequences and verification

Callbacks can query manager IDs and declarations after the caller releases its
registry guard. Independent channels do not share an operation lock. A callback
recursively awaiting mutation of its own channel remains unsupported; holding an
entry read lock while recursively awaiting a write can also deadlock. Callers must
not retain any external manager guard across an owned-operation await.

Regression tests cover all five callback boundaries, batch snapshots and lazy
side effects, independent entries, failed rollback, cancelled start/close, handler
retry, mutable-access epochs and the declared metadata behavior. Existing channel
examples retain the borrowed API and remain in workspace example compilation;
SDK adapter, three-language Host proofs and final exact-source gates are separate
consumer evidence. New Rust APIs do not expand the accepted cross-language wire
contract. Website publication follows the verified framework delivery snapshot.
