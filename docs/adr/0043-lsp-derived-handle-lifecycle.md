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
A fresh manager lifecycle is required to start new children.

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
changes. Finding #64 remains open for restart counters, EOF state, and config
reload semantics.

## Rollback

Revert the manager/client fence and focused tests, restore the previous LSP
lifecycle docs, and rerun the LSP feature tests. The rollback must not remove
the Finding or claim independent client ownership is safe.

## References

- `.echo-semantic/findings/finding.lsp-manager-derived-handle-resurrection.md`
- `.echo-semantic/findings/finding.lsp-runtime-state.md`
- `docs/en/31-lsp-integration.md`
- `docs/zh/31-lsp-integration.md`
