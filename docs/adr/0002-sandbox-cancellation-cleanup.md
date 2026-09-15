# ADR 0002: Sandbox Cancellation Cleanup

## Status

Accepted

- Date: 2026-08-25
- Updated: 2026-09-15
- Owners: `echo-core/sandbox`, `echo-execution/sandbox`

## Context

`SandboxExecutor::execute_with_limits_and_cancel` provides a default cancellation
race for lightweight implementations. `LocalSandbox` previously inherited that
default even though its actual child process is owned by a detached streaming
task. The default returned `SandboxError::Cancelled` as soon as the caller's
future was dropped, while the detached task was still observing receiver close,
killing the process group, and waiting for the child. A terminal result therefore
did not prove that local execution had reached a cleanup safe point.

Two additional drop windows existed. Local stdin was written before the child
was transferred to the detached stream task, so aborting a blocked write only
dropped the process handle. Docker lifecycle state remained inside the caller's
future, so aborting after `docker create` detached neither a cleanup owner nor an
RAII receipt and could leak the container. Docker also trusted create stdout as
its only cleanup identity, even when that output was empty or malformed.

`DockerSandbox` did own its cancellation branch, but every terminal path ignored
the spawn result and exit status of `docker rm -f`. Normal completion, timeout,
cancellation, or an execution error could be returned while the container still
existed, with no typed indication that cleanup failed.

`K8sSandbox` had the same ownership gap around `kubectl run --attach`. Dropping
the caller only dropped the local kubectl future; a Pod already accepted by the
API server had no remaining owner that would issue and settle deletion. Every
terminal path also ignored the spawn and exit result of `kubectl delete`, so a
successful-looking execution result did not prove that the Pod API object had
gone or that finalizers had settled.

## Options Considered

1. Keep best-effort cleanup after returning the primary terminal. This minimizes
   latency but makes terminal state unreliable and can leak processes or
   containers into later runs.
2. Add a second application-level cleanup supervisor. This duplicates sandbox
   ownership in each embedding product and cannot reliably identify the exact
   process or container created by the backend.
3. Transfer each spawned resource immediately to one detached backend owner and
   make that owner await cleanup before choosing a terminal. This keeps lifecycle
   authority beside resource creation and survives caller task abort.
4. Replace direct Pods with Kubernetes Jobs and rely on
   `ttlSecondsAfterFinished`. This adds a useful cluster-side crash fallback, but
   changes the execution protocol to a Job/Pod controller lifecycle. Deleting a
   running child Pod alone can also prompt its active Job to create a replacement,
   so cancellation would have to settle foreground Job deletion rather than the
   existing direct-Pod contract.

## Decision

Resource cleanup is part of controlled sandbox execution's terminal contract.

- `LocalSandbox` overrides `execute_with_limits_and_cancel`. Immediately after
  spawn it captures the process-group ID and transfers the child, stdin payload,
  output pipes, deadline, and cancellation token to one detached owner. The
  owner signals the captured group, awaits the leader, and verifies that signal
  zero returns `ESRCH` before it chooses a terminal. Cancellation during blocked
  stdin delivery uses the same owner and cleanup order.
- Buffered local execution retains the owner task join handle and awaits it.
  Live streaming remains receiver-owned: dropping the stream closes the channel
  and wakes the detached owner task to perform kill and wait.
- Local cleanup debt never becomes a successful-looking `Complete`. An active
  stream receives `SandboxStreamEvent::Failed` with a typed
  `SandboxStreamFailure`; a dropped stream has no receiver, so the owner retains
  the typed debt and logs cleanup failure before exiting.
- `DockerSandbox` rejects pre-cancel before probing or creating. It preallocates
  a unique `--name`, then transfers create, start, stdin, client process, output,
  timeout, cancellation, and `rm` to one detached lifecycle owner. Caller abort
  drops an armed guard that signals a separate abandonment token; the owner
  enters the same cancel cleanup path instead of waiting for the user timeout.
  Empty or malformed create stdout still cleans by the preallocated name.
- Docker normal completion, non-zero exit, timeout, cancellation, start failure,
  stdin failure, and Docker I/O failure all attempt `docker rm -f` before return.
- Docker cleanup checks command spawn and exit status. Cleanup failure becomes a
  typed `SandboxError::IoError`; when execution also failed, the bounded terminal
  message preserves exit, timeout/cancel, output-count, and cleanup facts.
- Docker `info`, `create`, and `rm` use owned CLI children with fixed control
  deadlines and kill/wait cleanup. Named removal retries are bounded to cover a
  daemon commit racing an interrupted `create`. Availability hits a clone-shared
  30-second cache; only a cache miss launches the owned `info` probe. Global
  cleanup attempts every fully parsed container and returns one bounded aggregate
  after all attempts; a truncated listing is always a typed incomplete failure.
- Docker output readers drain incrementally into one shared retained-byte budget
  while saturating logical byte counts. Normal, timeout, and cancellation paths
  cannot allocate retained output beyond `max_output_bytes`.
- `DockerConfig::extra_args` uses a narrow allowlist (`user`, `hostname`,
  `platform`, `entrypoint`). Owner and isolation flags such as name, label,
  restart, network, namespace, mount, security, and capability settings are
  rejected in both split and `=` forms.
- Unix Local cancellation owns process groups. The current Windows backend has
  no Job Object implementation, so every Local execution entry point reports
  unavailable before spawn instead of claiming that killing `cmd.exe` cleaned
  its process tree. Windows consumers must use Docker/K8s.
- `K8sSandbox` preallocates the Pod name and transfers kubectl, stdin, output,
  execution deadline, cancellation, Pod identity, and deletion to one detached
  backend owner. A caller-abandonment guard wakes that owner, which kills and
  reaps the kubectl process group before it deletes the named Pod. A normally
  exited kubectl leader also triggers process-group settlement so a helper that
  inherited stdout/stderr cannot hold output drain open. Pipe drain observes the
  remaining execution deadline and caller-abandonment token. Normal exit,
  non-zero exit, stdin or kubectl failure, timeout, cancellation, and caller drop
  therefore converge on the same cleanup path.
- Pod deletion uses a one-second graceful period and an explicit bounded
  `--wait=true`. This waits for the API object to disappear, including finalizer
  settlement. It deliberately does not use `--force --grace-period=0`, because
  Kubernetes documents that force deletion removes the API object without
  confirmation that the process on the node has terminated.
- A kubectl deletion spawn failure, timeout, or non-zero exit is cleanup debt.
  An awaiting caller receives a typed sandbox I/O error containing bounded
  primary terminal facts plus the cleanup failure. The detached owner records
  cleanup debt before it hands the terminal to the caller, removing the
  owner-result/caller-ack loss window. Join-failure compensation is itself a
  detached cleanup task, so dropping its waiter cannot interrupt Pod deletion.
- Label-wide K8s cleanup checks both listing and deletion status and uses the
  same bounded graceful wait. The kubectl executable remains a private backend
  indirection so deterministic fault tests do not mutate process-global `PATH`.
- Job/TTL cleanup remains a possible cluster-crash fallback, not the terminal
  authority for this direct-Pod repair. Adopting it requires a separate protocol
  decision that atomically changes creation, attachment, cancellation, owner
  references, foreground propagation, and TTL behavior.
- The default trait implementation remains available only for executors whose
  resources are fully released by dropping their execution future.

No EKO policy, UI field, or application lifecycle state is introduced. This is a
framework execution invariant shared by every embedding application.

## Consequences

Cancellation and timeout may take slightly longer because return now includes
cleanup latency. In exchange, callers can treat the returned terminal as a safe
point for subsequent execution, shutdown, or workspace reuse. Cleanup failures
are observable and retryable rather than silently converted into apparent
success.

`SandboxStreamEvent` gains a terminal `Failed` variant and the public
`SandboxStreamFailure` type. Exhaustive stream consumers must handle it separately
from `Complete`. Local execution is unavailable on Windows until a Job Object
owner is implemented.

The Docker executable indirection is private and exists so backend command
status can be tested deterministically without requiring a live Docker daemon or
mutating process-global `PATH`.

The K8s terminal can now take up to its bounded cleanup control interval after
the primary command settles. A finalizer, unreachable node, or API failure is
reported instead of hidden. Caller drop survives while the Tokio runtime remains
alive; a full process or runtime crash still needs cluster-side reconciliation,
which is outside this direct-Pod settlement.

## Verification

Regression coverage starts a real local leader and descendant, waits until the
leader exits, cancels execution, and verifies that the captured group and
descendant are absent at terminal return. A blocked-stdin test aborts the caller
and verifies that the detached owner still cleans the group. Typed failure tests
prove cleanup debt cannot emit a fake completion.

A fake Docker executable verifies pre-cancel performs no Docker operation;
normal, non-zero, timeout, cancellation, blocked stdin, and caller-abort paths
converge through `info -> create -> start -> rm`; empty/bad create output still
uses named cleanup; and non-zero/timeout/cancel facts survive a simultaneous
non-zero cleanup failure. Additional faults cover hung info/create/rm stages,
cleanup retry, first-failure global cleanup continuation, reserved extra args,
and bounded high-volume output for normal/timeout/cancel terminals.

A fake kubectl executable verifies successful and non-zero completion, timeout,
cancellation, blocked/failed stdin, leader-exit with an inherited output pipe,
and caller abort all reach settled Pod deletion; deletion is graceful, bounded,
and waits for API disappearance. Cleanup failure preserves success, non-zero,
timeout, and cancellation facts instead of returning a false terminal, and
detached join-failure recovery survives loss of its waiter.

## References

- Kubernetes, [Pod lifecycle and termination](https://kubernetes.io/docs/concepts/workloads/pods/pod-lifecycle/#pod-termination-flow)
- Kubernetes, [Finalizers](https://kubernetes.io/docs/concepts/overview/working-with-objects/finalizers/)
- Kubernetes, [Cascading deletion](https://kubernetes.io/docs/concepts/architecture/garbage-collection/#cascading-deletion)
- Kubernetes, [Jobs and TTL cleanup](https://kubernetes.io/docs/concepts/workloads/controllers/job/#clean-up-finished-jobs-automatically)
- Kubernetes, [`kubectl delete`](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_delete/)
