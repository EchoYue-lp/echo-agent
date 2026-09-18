# ADR 0043: Manager-owned LSP derived handle lifecycle

## Status

Accepted

Accepted on 2026-09-15.

## Context

`LspManager` owns language-server child processes, but its public `get_client`
methods currently return an `Arc<RwLock<StdioLspClient>>` with no parent or
generation link. A caller can retain that derived handle after manager close
and call `initialize`, allowing the client to spawn a child outside the
manager's ownership tree. This is Finding #63 (Issue #63) and is coupled to,
but does not solve, the separate runtime-status work in Finding #64.

## Decision

Use manager-level cascade invalidation. The manager owns a shared lifecycle
fence. Every client created by the manager captures that fence and its current
generation. Manager shutdown closes the fence before awaiting owned child
shutdown. Derived handles remain usable only while their captured generation is
live; after close they return the existing typed `LspError::NotInitialized`
stale/closed error and cannot initialize, send requests, or send notifications.
A fresh manager lifecycle is required to start new children after manager close.

The existing `Arc<RwLock<StdioLspClient>>` facade shape remains unchanged. The
SDK Host continues to own only facade records and calls `shutdown_all`; it does
not become a second process owner.

## Alternatives considered

1. **Independent client ownership**: rejected because it permits child
   processes to outlive the manager and creates split cleanup authority.
2. **Remove all derived handles on close only**: rejected because callers can
   retain an `Arc` that the registry no longer knows about.
3. **Manager generation fence with cascade invalidation**: selected because it
   preserves the existing API shape, gives deterministic stale-handle behavior,
   and keeps one process owner.

## Consequences

Consumers must reopen a manager to obtain usable clients after a manager close.
Stale handles fail explicitly instead of silently reviving a child. The
framework gains one internal lifecycle token; no wire schema or SDK operation
changes.

## Runtime status and configuration follow-up (Issue #64)

The client's shared runtime snapshot is the authority for live transport state.
Its stdout reader clears `running`, `initialized`, and PID on EOF or framing
failure, drops pending calls and cached diagnostics, and records the cause.
Manager status retains restart attempts and the last error after client removal.
An explicit `restart_server` consumes one configured `max_restarts` attempt;
initial start and deliberate repeated `start_server` do not consume that budget.
An exhausted budget returns an error without starting a process. There is no
background retry policy or implicit resurrection of a dead child.

Synchronous `load_config` merges cold configurations, but rejects a manager
that still owns client entries. `reload_config` awaits all old child shutdowns,
then replaces the complete configuration and extension routes; callers must
start desired servers explicitly. A retained client is closed before the new
route becomes visible. A closed manager rejects both configuration methods.

We considered making synchronous reload enqueue asynchronous teardown, but it
would publish new routes before old process settlement and hide cleanup errors.
Making every configuration call asynchronous would break cold-start consumers
that require no teardown. The separate asynchronous replacement makes the
resource boundary explicit while retaining the cold-start API. The LSP 3.17
shutdown/exit ordering and Tokio's explicit cancel-and-wait shutdown pattern
inform the bounded graceful request followed by process termination and bounded
reader/writer task joins; `Drop` does not establish completion.

## Rollback

Revert the manager/client fence and focused tests, restore the previous LSP
lifecycle docs, and rerun the LSP feature tests. The rollback must not remove
the Finding or claim independent client ownership is safe.

## References

- `.echo-semantic/findings/finding.lsp-manager-derived-handle-resurrection.md`
- `.echo-semantic/findings/finding.lsp-runtime-state.md`
- `docs/en/31-lsp-integration.md`
- `docs/zh/31-lsp-integration.md`
- https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#shutdown-request-leftwards_arrow_with_hook
- https://tokio.rs/tokio/topics/shutdown
