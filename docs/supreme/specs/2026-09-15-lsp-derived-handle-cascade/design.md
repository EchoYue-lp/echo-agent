---
title: LSP derived handle cascade invalidation
artifact: design
carrier: markdown
---

# LSP Derived Handle Cascade Invalidation

## Problem and goal

`LspManager` owns every language-server child process. Today `get_client` and
`get_client_for_file` clone an `Arc<RwLock<StdioLspClient>>` without retaining a
link to that owner. A client handle can therefore outlive the manager and call
`initialize`, spawning a new child after the manager has been closed.

The goal is to make the manager the only child-process owner and make every
derived client handle stale after manager shutdown. A stale handle must reject
`initialize` and every operation that could communicate with or revive a child;
it must never spawn a new process independently.

## Target behavior

- `LspManager::get_client` and `get_client_for_file` return a derived handle
  carrying the manager's lifecycle fence.
- `LspManager::shutdown_all` invalidates the fence before or together with
  shutting down owned clients. Existing handles observe the invalidation even
  when they remain strongly referenced by a caller.
- A stale handle returns the existing typed `LspError::NotInitialized`
  lifecycle error from `initialize`; it does not invoke `spawn_process` or
  require a new SDK wire variant.
- A stale handle cannot send requests or notifications through a previously
  closed client. Existing live handles continue to use the same child until
  manager shutdown.
- A manager may be explicitly reused only by constructing a fresh manager
  lifecycle. Restarting an individual language server under the same live
  manager creates a fresh client bound to the current fence; it does not revive
  a stale derived handle.

## Decision

Use manager-level cascade invalidation. The manager owns a shared lifecycle
fence, every derived client captures it, and shutdown closes the fence before
child teardown. The existing client handle shape stays intact; the fence is an
internal framework authority rather than a new SDK resource type.

## Scope and non-goals

In scope: the framework LSP manager/client lifecycle, the SDK Host's existing
manager shutdown path, focused lifecycle tests, and the LSP lifecycle docs.

Out of scope: the separate LSP runtime status/restart accounting Finding #64,
MCP cleanup Finding #55, provider or Hook behavior, SDK operation renaming, and
application-specific resource policy. No second LSP registry or independent
client owner is introduced.

## Ownership and boundary

The framework owns the child process, client lifecycle, and manager-generation
fence. SDK Host owns only its facade resource records and calls the manager's
existing shutdown authority during resource/session close. The facade continues
to expose the existing `Arc<RwLock<StdioLspClient>>` shape so protocol and SDK
contracts do not acquire a second lifecycle authority.

## Core structure and data flow

Each manager has an internal shared lifecycle token containing a monotonic
generation and a closed flag. Every client created by `start_server` captures
that token and its generation. `get_client*` only returns clients from the
manager's current set. `shutdown_all` marks the token closed, then awaits each
owned client's shutdown and removes the manager entries. Client entry points
check the token before spawning or sending data.

The authoritative sequence is:

```text
manager open/live
  -> start_server creates client(bound to fence generation)
  -> get_client derives Arc handle
  -> shutdown_all closes fence
  -> owned clients shutdown and are removed
  -> derived handles remain stale and reject initialize/IO
```

## Failure and concurrency cases

- A handle retained across shutdown fails deterministically with the closed
  error; no child process is created.
- A concurrent `initialize` racing shutdown must re-check the fence after any
  await boundary before spawning. Shutdown winning the fence closes the
  operation; it must not leave an unowned child.
- Repeated `shutdown_all` is idempotent. It keeps the fence closed and does not
  resurrect or duplicate children.
- A failed initialization cleans up the just-spawned child through the existing
  client shutdown/drop path and does not publish it into the manager map.
- Manager-level status remains authoritative for owned clients. A stale
  derived handle is not reported as a running manager server.

## Reuse and implementation constraints

- Reuse `LspManager`, `StdioLspClient`, `LspClient`, and the existing facade
  handle registry. Do not add a parallel client wrapper or map.
- Keep the lifecycle fence internal unless a public typed error is required by
  the existing `LspError` contract. Do not expose manager internals in SDK
  wire values.
- Preserve the framework/application boundary: process ownership stays in
  `echo-integration`; SDK Host remains a thin resource adapter.
- Update the bilingual LSP lifecycle docs and the existing architecture ADR;
  no learning example behavior changes are needed because the public calls are
  unchanged.

## Acceptance criteria

1. A derived client retained before `shutdown_all` returns the closed/stale
   error from `initialize` and does not create a child process.
2. The same handle cannot issue LSP requests or notifications after shutdown.
3. Shutdown closes the manager fence before awaiting child teardown and is
   idempotent.
4. Live manager start/stop/restart behavior remains green for existing tests.
5. SDK Host session/resource close still invokes `shutdown_all` and leaves no
   owned client in its resource maps.
6. Focused tests, formatter, clippy, semantic-diff, and semantic-verify pass;
   the full workspace gate remains the merge-owner's responsibility.
