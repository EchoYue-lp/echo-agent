# ADR 0034: Context-scoped Tool result cache

- Date: 2026-09-13
- Owners: `echo-execution/tools`

## Status

Accepted.

## Context

`ToolManager` caches successful `ReadOnly` results for a short bounded period.
The original key contained only the Tool name and canonical parameters. A
manager shared by multiple Agent invocations could therefore reuse a relative
file read or another context-sensitive result across workspaces, runs, turns,
or Subagent attempts.

Every non-read call cleared the cache before acquiring its execution permit.
That did not fence reads already in flight, nor reads that started while the
write was pending. Such a read could observe pre-write state and insert it
after the write completed, making a stale value current again.

RFC 9111 defines a cache key as all information needed to select a reusable
response and requires state-changing requests to invalidate stored responses.
ToolManager is not an HTTP cache, but those two principles apply to its local
read-result cache:

- <https://www.rfc-editor.org/rfc/rfc9111.html#section-2>
- <https://www.rfc-editor.org/rfc/rfc9111.html#section-4.4>

## Options Considered

1. Disable result caching whenever `ToolContext` is non-empty. This prevents
   contamination but removes useful repeated reads in the same invocation.
2. Add only workspace identity to the key. This separates worktrees but still
   shares results across run/turn/attempt contexts and does not stop in-flight
   publication after a write.
3. Serialize every read behind the write semaphore. This closes the race but
   discards the deliberate higher read concurrency and lets unrelated scopes
   block one another.
4. Extend the existing key with stable context identity and add one monotonic
   invalidation epoch guarded by the existing cache lock.

## Decision

Choose option 4.

- `ToolResultCacheKey` contains the Tool name, canonical parameter JSON,
  effective working directory, and the optional conversation, run, turn,
  message, and execution identifiers from `ToolContext`. When an output
  artifact policy is present, the key also contains its effective root,
  retention, threshold, and maximum age because built-in read Tools use those
  values to select or shape their result.
- An absolute bound working directory is used directly. A relative bound path
  is resolved against the process current directory. When no working directory
  is bound, the current directory is the scope. If it cannot be read, caching
  is disabled for that invocation rather than using an ambiguous key. Relative
  artifact roots are resolved separately against the process current directory,
  matching `ToolOutputArtifactWriter` and `ReadArtifactTool`; they are not
  rebased under the bound Tool working directory.
- `call_id` is excluded. It identifies one physical call and its retries;
  including it would prevent repeated logical reads inside the same execution
  from sharing a result. Message and execution identity still fence distinct
  calls whose context can differ.
- The existing `result_cache` remains the only result cache. Its TTL and
  capacity do not change.
- `result_cache_epoch` is monotonic. A read captures it before lookup and may
  insert a successful result only while holding the cache write lock and only
  when the epoch still matches.
- Every non-read Tool call holds a `ResultCacheInvalidationGuard`. Construction
  acquires the cache lock, increments the epoch, and clears entries. Drop does
  the same on success, failure, cancellation, timeout, retry exhaustion, or
  caller drop. Reads that overlap any part of that lifetime either have their
  entry cleared at the end or fail the conditional insert.
- Tool registration, batch registration, replacement, removal, and explicit
  definition-cache refresh also increment the result epoch and clear entries.
  Execution holds the current DashMap Tool guard through result publication, so
  replacement waits for that publication and then invalidates it. Calls also
  observe the epoch before obtaining the guard, preserving the fence if Tool
  registry storage later stops serializing replacement behind an active call.
- Streaming and non-streaming execution use the same key, epoch, guard, lookup,
  TTL, capacity, and conditional store operations.

## Consequences

- Equal Tool parameters are reusable only inside the same effective workspace
  invocation lineage, artifact policy, and Tool registration generation.
- A changing Tool invalidates conservatively even when it fails, because a
  partial external effect may already have occurred.
- Reads remain concurrent. Cache freshness does not require serializing all
  reads behind the write semaphore.
- Calls without a readable effective directory still execute normally but do
  not cache their result.
- Equivalent paths with different lexical spellings can produce separate
  entries. This is a cache miss, not cross-scope reuse; filesystem
  canonicalization is intentionally avoided because the target may not exist.

## Compatibility And Rollback

Public methods, `ToolContext`, `Tool`, result/error types, stream events, wire
formats, and SDK identities do not change. Consumers can observe fewer cache
hits across runs or workspaces and no longer observe a read value resurrected
after a write.

The repair is isolated to ToolManager cache internals and can be rolled back by
reverting its single commit. It adds no persistent data or migration.

## Verification

Deterministic tests isolate workspace, run, execution, message, and artifact
configuration dimensions while preserving same-scope hits. Artifact tests also
distinguish a relative root from the same lexical suffix under the Tool working
directory. Replacement tests cover both a completed cache entry and an old
implementation blocked in flight while replacement begins. Another test starts
a read that captures value 0, executes a write that changes it to 1, then
releases the old read; the next read must execute again and return 1. Existing
cache capacity, write invalidation, streaming, validation, retry, timeout,
cancellation, and drain tests remain in the focused suite.
