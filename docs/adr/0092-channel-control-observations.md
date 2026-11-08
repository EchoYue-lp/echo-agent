# ADR 0092: Observe original channel transports and replace only settled adapters

## Status

Implementation candidate for confirmed Plan07 (`plan:1bd5cb90-ae0e-4399-b0ee-0ccd9dbfa60c`, SHA-256 `51d54ea29d39790508296b74b0a1420d715b5f2347043c3187f395382a0caac6`). The source starts at `e4e50396876c5e7e85b52cdc3dbb510bab9c4f4d`. Focused local Rust validation is recorded below; this candidate is not evidence of platform login, real IM delivery, EKO integration, full feature/main gates or release. Those checks remain separate.

## Context

`ChannelManager` already owns one entry for each original plugin. Its owned handle retains the same handler, invocation generation, cancellation and stop/close responsibility described in [ADR 0085](0085-channel-handler-invocation-ownership.md). QQ and Feishu start background transports before remote readiness is known. A token refresh, successful start or health check cannot establish a QQ session, a Feishu WebSocket handshake, webhook platform reachability or delivery.

An application needs to apply changed channel credentials or receive modes without creating another manager or discarding an unsettled handler. Registration metadata remains immutable. The former transport stop could take a join handle before awaiting it, losing the original join owner if the caller cancelled that waiter. Independently spawned heartbeats could also outlive a cancelled outer connection future.

## Alternatives

1. Infer connected from `start` or `health_check`. Rejected: neither proves the required transport fact.
2. Add a connection manager, durable status store or retry controller for presentation. Rejected: these duplicate the existing adapter loop and entry authority.
3. Remove and register a new plugin after any stop attempt. Rejected: this can drop cleanup debt, captured invocations and stop receipts.
4. Extend the original adapter with optional latest-value facts and the original entry with a settled replacement operation. Selected by Plan07.

The approved plan compares [Tokio watch](https://docs.rs/tokio/latest/tokio/sync/watch/index.html) with a new event store: watch retains only the latest observation and receiver drop does not own the service. [Tokio JoinHandle](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html) distinguishes detaching from joining. Here heartbeat is driven as a future inside the original transport task, and the adapter retains its actual task handles until joins complete. No new scheduler or manager is needed.

## Decision

### Generic mechanism and application policy

The framework owns transport facts and the original entry lifecycle. EKO owns configuration persistence/revisions, applying a selected configuration, serializing controls, UI projection, private test targets and the two enabled product channels. The handle is a thin bridge; these Rust additions do not enlarge the independently accepted SDK wire/language contract.

`ChannelPlugin::transport_status()` defaults to `None`, meaning unsupported/unknown. Built-in adapters return a watch of `ChannelTransportSnapshot` containing `phase`, optional typed `evidence`, redacted `last_error` and observation time. It is not a history or another state authority, admits no work and never drives retries. Each built-in start replaces its observation source. An abandoned old receiver is not redirected to a new connection; a closed receiver does not prove stop success. Management projection turns a closed source into Unknown even if its retained last value was Ready; this does not change handler or stop authority.

Only the original protocol boundary publishes Ready:

| Adapter | Evidence |
| --- | --- |
| QQ | An authenticated dispatch reports READY or RESUMED; TCP/WS connection, HELLO and IDENTIFY alone stay Connecting |
| Feishu long connection | The original `connect_async` handshake succeeds, after endpoint acquisition |
| Feishu webhook | The original `TcpListener::bind` succeeds; this is only `LocalWebhookListener`, not platform reachability |

QQ HELLO with `heartbeat_interval = 0` returns a typed protocol error before timer construction; every positive interval remains valid without a new minimum. Original loop exits, retry branches and failures publish Disconnected/Failed as appropriate. Unexpected task destruction publishes Unknown. Built-in errors remove exact configured secrets before general retention redaction. No credential, event payload or endpoint authentication value is added to observations. Send receipts remain independent: the platform API accepting a send is not a recipient read receipt.

Owned `ChannelHandle::transport_status().await` returns `ChannelTransportObservation { mutation_epoch, receiver }`. Compare that epoch with the current management snapshot before projecting an update after control. The trait method on a borrowed handle cannot await the original lock; it returns None while busy. Owned access is preferred for control consumers.

`ChannelHandle::management_snapshot().await` projects only existing entry facts: mutation epoch, handler attachment, active invocation count, the latest adapter observation and `shutdown_settled`. Settled requires a confirmed stop at the current non-exhausted epoch, no handler and no active invocation. Transport Stopped with failed handler close remains unsettled; health is never called by these read operations.

`replace_stopped_plugin` validates the same id, label and complete capability declaration, then holds the original lifecycle lock. It rejects without modifying the old entry unless the original stop receipt is current, handler absent, invocation generation retired and empty, and checked epoch increment succeeds. It invalidates old stop/capture facts and swaps only the adapter, without a fallible await after installation. The new adapter must still use original start/rollback/stop. Start advances the same checked epoch; exhaustion fails conservatively rather than wrapping. Mutable borrowed access retains its pre-existing conservative invalidation behavior.

### Physical cleanup and failure

QQ and Feishu WebSocket heartbeats are sibling futures inside the original connection task. Cancellation or unwind cannot detach a separate heartbeat task. Their existing reconnect loops stay authoritative. Adapter stop signals the original transport cancellation, awaits saved task handles by mutable reference and removes them only after actual join. Caller cancellation retains those handles for retry. A join failure remains recorded through any later await in that same adapter until the completed stop reports it. A failed stop or handler close never authorizes replacement.

Webhook stop uses the original Axum server's graceful shutdown, closing listener admission and waiting for admitted requests. Pending application handlers can therefore keep stop pending; neither window disappearance nor observer cancellation is treated as physical completion. No extra timeout or forced process exit is introduced. Webhook configuration keeps the locked Axum 0.7.9 grammar, including `/events/:tenant` and `/events/*tail`. Only its own leading-slash requirement and the same fallible matchit parser reject malformed paths before calling the panic-prone public route constructor; no literal-only policy is added.

### Reuse the already locked route parser

The complete source search found no framework fallible route validator. Axum 0.7.9 `routing/mod.rs:164-168` and `route_service` convert insertion errors to panics; `routing/path_router.rs:42-94` is private, its module is `pub(crate)`, and Axum does not publicly re-export matchit. That private entry validates nonempty/leading slash, then `Node::insert` (`path_router.rs:441-460`) delegates to `matchit::Router::insert`.

The minimal explicitly approved edge is `echo-integration/Cargo.toml`'s existing `channels` feature adding `dep:matchit`, with `matchit = { version = "=0.7.3", optional = true }`. Cargo.lock already contains Axum's matchit edge and matchit 0.7.3, registry source `registry+https://github.com/rust-lang/crates.io-index`, checksum `0e7465ac9959cc2b1404e8e2367b43684a6d13790fe23056cc8c6c5a6b7bcb94`. The only lock edit is adding `"matchit"` to `echo_integration.dependencies`; no package, version, source or checksum changes. This reuses the same parser instead of copying its grammar or catching a panic.

Lock SHA-256 before the added edge: `f76fe10e64ec3faf062ff5bc7a66d6cd82f82cf95e989c824f40fe6f244d234c`; after: `4cdd98547fe19f58fb8d7fe0fe5c54b6342bf0b2f007f9d2591b47a993ad21d1`. These are source identities, not completed Cargo validation or automatically renewed accepted dependency-policy evidence. The changed optional edge requires the root facade/independent-feature matrix and fresh exact dependency-policy revalidation under the coordinator's validation schedule. EKO/SDK locks and policies remain owned by those repositories.

## Consequences

Public Rust values/methods are additive and external plugins remain source compatible through the default trait method. Only the explicitly approved optional edge to the already locked matchit parser changes a manifest/lockfile; no permission gate, credential storage, framework HumanLoop/InboundMessage field, configuration policy, manager, store or retry policy is introduced. SDK acceptance and EKO composition belong to their owners. The framework facade continues to re-export this same integration implementation.

The focused verification must cover default unknown behavior; real controlled loopback WS/QQ READY and bind boundaries; observer drop and old-source isolation; stopped replacement and metadata/epoch rejection; start/rollback/handler-close debts; preserved invocation capture and cancelled stop waiters retaining join ownership. A gated real loopback webhook regression uses the original FeishuChannel and ChannelHandle, local token/reaction fixture and captured route. It polls actual stop to pending, drops that waiter, verifies the original handler remains attached and unsettled, polls retry before release (which must still wait for the original join), then releases the HTTP handler and requires the actual HTTP receipt, joined stop and exactly one handler close. Additional controls cover active versus dropped external watch sender, invalid versus legal locked routes, and QQ HELLO zero versus positive interval. These definitions were subsequently executed in the focused local batch below. Existing channel/session/delivery tests and channel doctests remain part of validation. No real platform was contacted.

### Focused local validation and remaining boundaries

The `echo_integration` no-default-features `channels` locked check, the corresponding root `echo_agent` facade check, integration all-target Clippy with `-D warnings`, and non-test lib Clippy with strict unwrap/expect/panic/unreachable lints all returned zero. The original complete `channels::` lib selection executed 103 tests: all 23 new boundary cases and 80 existing controls passed, none ignored; 96 unrelated lib cases were filtered out by that explicit channel scope. Two existing channel doctests compiled successfully.

The first collection had one failing new Feishu handshake fixture. It read a published Connecting version without marking it seen, so its next `changed()` observed that prior notification before the gated handshake. The same case reproduced before a minimal test-only `borrow_and_update` correction. The repaired case and complete original collection then passed with the exact Connecting/Ready/evidence/socket assertions preserved. The initial natural 101 outputs remain alongside the final zero receipts; production transport behavior did not change in that repair.

The actual raw logs, selected-case proof and cleanup receipts live in this task's ignored `.echo-semantic/local/channel-control-observations/` directory. After saving needed evidence and confirming no target readers, the batch cleaned its own target. These focused results do not replace the independent feature matrix, workspace/main gates, real platform/GUI acceptance or fresh exact dependency-policy binding for the new lock. The original baseline lock hash is not automatically renewed by matching package identities.

Existing examples use the same start/stop APIs; no example requires API migration. The bilingual IM guide is updated here. Website mirrors must later take this exact reviewed framework contribution through the website owner; source documentation is not a website deployment receipt.
