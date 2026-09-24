# ADR 0072: Exact Resource Cleanup Ownership

## Status

Accepted

- Date: 2026-09-24
- Owners: `echo-core/tools/artifact`, `echo-execution/sandbox`, `echo-tools/git_worktree`

## Context

Three reusable framework effects lacked truthful cleanup settlement. Explicit
artifact scope deletion returned success while a writer was active, and a later
deletion failure was only logged. Age-based artifact sweeping could delete a
live writer's partial file. `SandboxManager::cleanup` reached Docker and K8s
label-wide sweeps, although one manager does not own every resource with that
shared label. A Git worktree could be created before its ownership marker was
written; cancellation or marker failure left an unmarked checkout.

EKO owns when a conversation is deleted or a manager is shut down. These
resource identity and settlement rules apply to every framework consumer.

## Options Considered

1. Treat deferred cleanup and `Drop` as sufficient. This hides deletion errors
   and makes a successful return mean two different things.
2. Give the application a global label or filesystem sweep. This can delete
   another owner's live resource and cannot settle an ambiguous Git add.
3. Keep each effect's identity with its creator, expose pending/debt explicitly,
   and compensate only a proven exact, clean checkout. This preserves the
   existing component boundaries and provides a retry point.

## Decision

- `cleanup_tool_output_scope` keeps its public signature. It returns
  `WouldBlock` while any writer in the requested scope is active and retains
  the exact pending root. Writer release performs deferred deletion; a failure
  remains `Retryable` and can be retried through the same call. The new
  `tool_output_scope_cleanup_state` reports `Settled`, `Pending`, or `Retryable`.
  Age sweeping skips active scope files and directories. Asynchronous
  finalization transfers the writer to one blocking task until flush, sync,
  rename, and scope release complete; absence of a Tokio runtime returns an
  error. Writer construction creates its root directory and binds the writer
  to its canonical path plus physical file identity before registration (Unix
  device and inode; Windows volume and file index). It reuses the framework's
  `ExistingDirectoryGuard`, retaining that directory handle through pending
  cleanup debt so a replaced path cannot reuse the original identity. Later file creation and
  age sweeping reject a changed physical root. A cleanup request made while a
  writer is active retains that same identity through deferred deletion and
  retry. Explicit cleanup recognizes the original alias of an active writer,
  even if the alias now points elsewhere. A missing or changed root cannot
  authorize deletion of a replacement. The application still decides retention
  and when to request scope deletion.
- Each Docker/K8s backend instance shares an exact-name active/debt registry
  with its lifecycle-owner clones. Per-command cleanup removes a settled name
  or retains a failed name as debt. Its `cleanup()` retries only its own debt
  and reports active owners; `SandboxManager::cleanup()` attempts every backend
  and aggregates failures. Framework Agent close calls the retained sandbox
  executor after its Turn drain and attempts both MCP and sandbox settlement
  even if one fails. A manager shared with another live Agent reports that
  owner's active resource as retryable debt; it does not delete it. Label-wide
  `cleanup_sandbox_containers` and `cleanup_sandbox_pods` remain explicit manual
  recovery operations and are
  never called by ordinary manager cleanup or Agent close.
- Worktree creation reserves a new path, then an independent bounded owner
  retains that checkout directory's guard before `git worktree add`. After add,
  it also retains the Git administration directory guard. The owner completes
  marker publication even if its caller departs. The caller acknowledges
  receipt acquisition; if its receipt is dropped before acknowledgement, the
  owner uses the normal managed, clean-worktree removal path. Both guards are
  checked before marker write and immediately before owned removal. Marker
  failure triggers `git worktree remove` only after confirming the guarded
  checkout and admin identities, branch, and clean status.
  Compensation never uses
  `--force` or deletes a branch. Ambiguous add and failed compensation return
  the path and branch as recovery facts; they do not guess that deletion is safe.

## Consequences

Consumers must treat `WouldBlock` as pending cleanup rather than a completed
deletion. Writer construction now creates an empty artifact root even if no
output crosses the spill threshold; failure is reported at the first attempted
spill because the constructor remains infallible. If the bound root is moved or
replaced, cleanup retains retryable debt; the path-based API cannot discover
the moved directory, so recovery must identify it explicitly. These are
safe-point checks, not atomic fences against an external process renaming a
directory in the interval between verification and a path-based filesystem or
Git operation. EKO's local threat model has no untrusted concurrent tenant;
framework code does not rename these roots during cleanup. A manager with
active execution may report unsettled Agent close until the backend owner
finishes, after which close can be retried. Process crashes still require
explicit recovery; the in-process registries cannot prove settlement across
restarts. A reserved
worktree path is rejected if it already exists, including an empty directory.

## References

- [Tokio `JoinHandle`](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html): dropping a handle detaches its task.
- [Tokio graceful shutdown](https://tokio.rs/tokio/topics/shutdown): cancellation requests and waiting for task settlement are separate steps.
- [Tokio filesystem](https://docs.rs/tokio/latest/tokio/fs/index.html): file operations use the blocking pool, so ownership must span the underlying operation.
- [Git worktree](https://git-scm.com/docs/git-worktree): ordinary remove refuses a dirty worktree; pruning and forced removal are separate operations.
- [tempfile `TempPath::close`](https://docs.rs/tempfile/latest/tempfile/struct.TempPath.html#method.close): explicit close reports deletion errors that `Drop` cannot.
