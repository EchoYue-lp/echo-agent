# ADR 0035: Owned handles for the Tool registry

- Date: 2026-09-14
- Owners: `echo-execution/tools`

## Status

Accepted.

## Context

`ToolManager` stores `Box<dyn Tool>` directly in a `DashMap`. Its public
`get_tool` returns a `DashMap::Ref`, and both streaming and non-streaming
execution retain that reference through validation, permit acquisition,
retries, timeout handling, and the complete asynchronous Tool call.

`replace` and `unregister` are synchronous registry mutations. They can block
while an execution holds the shard reference. On a Tokio current-thread
runtime, calling either mutation from the executor thread while the Tool future
is suspended can prevent the same executor from resuming that future and
releasing the reference.

The Tokio shared-state guide warns that a synchronous guard held across
`.await` can deadlock even when its type is `Send`, and recommends limiting the
guard to a non-async scope. DashMap documents that mutations such as `insert`
may deadlock while a map reference is held. OpenAI Codex stores Tool runtimes
as `Arc` values and clones an owned runtime handle before async dispatch.

- <https://tokio.rs/tokio/tutorial/shared-state#holding-a-mutexguard-across-an-await>
- <https://docs.rs/dashmap/latest/dashmap/struct.DashMap.html#method.insert>
- <https://github.com/openai/codex/blob/16537b20a5ec0ea9aa079f4ad4b0e30e8a9efacf/codex-rs/core/src/tools/registry.rs#L459-L463>

## Options Considered

1. Require callers to move every mutation to `spawn_blocking`. This leaks the
   registry's lock implementation into every embedding application and leaves
   `get_tool` able to create the same hazard.
2. Keep the Box registry and add a second Arc execution registry. This creates
   two Tool generation authorities and makes publication ordering ambiguous.
3. Return a boxed proxy that forwards to an Arc-backed Tool. The proxy must
   duplicate every current and future `Tool` method, making risk, validation,
   streaming, schema, and sandbox behavior prone to wrapper drift.
4. Store one `Arc<dyn Tool>` per registry entry and return owned Arc handles
   from lookup, replacement, and removal.
5. Replace the registry with an actor protocol. This avoids synchronous locks
   but turns all builder and query operations into messages and is larger than
   the confirmed lifecycle defect.

## Decision

Choose option 4.

- `DashMap<String, Arc<dyn Tool>>` remains the only Tool catalog authority.
- Registration and builder APIs continue accepting `Box<dyn Tool>` and convert
  it once to an Arc at the registry boundary.
- `get_tool` clones an Arc while the DashMap reference is in a synchronous
  scope, then returns the owned handle. No registry guard crosses an `.await`.
- `replace` and `unregister` return `Option<Arc<dyn Tool>>`. A caller may inspect
  or retain the displaced generation without keeping the registry locked.
- The inherent `ReactAgent::replace_tool` and `ReactAgent::remove_tool` methods
  follow the owned Arc return contract. The object-safe `Agent::remove_tool`
  method continues returning `bool`.
- A lookup racing with mutation may receive either the complete old or complete
  new generation. An old generation already in flight may finish normally;
  removal is not redefined as cancellation.
- Plan 12's result-cache epoch is observed before lookup. Every successful
  registry mutation advances that epoch and clears the cache, so a displaced
  Arc cannot publish a result under the replacement generation.

## Framework And Application Boundary

Owned Tool generation lifetime is a reusable framework primitive. It does not
depend on EKO workspace, UI, permission, reload, or product policy. Embedding
applications can decide when to mutate a registry, but they do not maintain a
parallel Tool catalog or generation counter.

## Consequences

- Synchronous lookup and mutation hold DashMap guards only for bounded map
  operations. Tool execution no longer couples executor progress to a shard
  lock.
- Old Tool resources are dropped after the registry and all in-flight or
  externally retained Arc handles release them. Async cleanup still requires a
  separate explicit lifecycle contract; Arc Drop is not a settlement receipt.
- Rust callers that explicitly require `Box<dyn Tool>` or DashMap `Ref` return
  values must migrate to `Arc<dyn Tool>`. Callers using `is_some` or Tool methods
  through dereference remain structurally equivalent.
- The affected SDK identities are classified `host_or_rust_only`. Rust public
  inventory signatures and digests change, while TypeScript, Python, Java,
  extension wire schemas, and facade operation counts do not gain new APIs.
- No dependency, persistence format, protocol, permission authority, or
  application state is added.

## Compatibility And Rollback

This is an intentional Rust source compatibility change required to remove a
deadlocking public lifetime. Registration inputs and the object-safe Agent
surface remain compatible. The dynamic Tool example and SDK inventory document
the migration.

Rollback means reverting the registry and public-return commit together with
its generated inventory. Reintroducing a borrowed map reference across async
execution is not an acceptable partial rollback.

## Verification

A bounded red command first demonstrates that the old implementation cannot
complete current-thread replacement while an active Tool holds the registry
reference. The repaired tests perform replacement and removal synchronously
while the old Tool is suspended, then release and settle the old generation.
They also prove that the replacement is visible, removal makes subsequent
lookup fail, and the old generation cannot repopulate the current result cache.

Existing ToolManager registration, cache, streaming, validation, retry,
timeout, cancellation, and drain tests remain green. The dynamic Tool example,
Rust public inventory generator, SDK contract checks, independent feature
checks, Clippy, semantic snapshot/change evidence, and Issue reconciliation are
also required.
