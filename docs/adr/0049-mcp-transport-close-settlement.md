# ADR 0049: MCP Transport Close Settlement

## Status

- Accepted
- Date: 2026-09-15
- Scope: `echo-integration::mcp`

## Context

`McpTransport` owns the I/O resources that carry one MCP connection. The legacy
SSE implementation registered a pending request before discovering the POST
endpoint, but several endpoint, POST, response-channel, and timeout exits did
not remove that registration. Its `close` method only cancelled the reconnect
token and returned without draining callers or awaiting the SSE task.

The stdio implementation failed pending callers on stdout EOF, but a stdout
read error did not do so. It did not retain its stdout or stderr task handles,
and `close` killed and waited for the child while pending callers and I/O tasks
could still be live. Kill and wait failures were logged or discarded because
the public close chain returned `()`.

The official MCP 2024-11-05 lifecycle specification defines transport shutdown
as a real lifecycle phase. For stdio it prescribes closing child input, waiting
for exit, and escalating termination after reasonable bounds; it also calls
out shutdown timeout handling. The official Rust SDK likewise exposes a
`Result`-returning transport close and implements bounded graceful child
shutdown. The official TypeScript SSE transport aborts outstanding fetches and
closes its event source. These implementations converge on an awaited,
fallible transport-owned close rather than a detached caller-owned cleanup.

References:

- <https://github.com/modelcontextprotocol/specification/blob/main/docs/specification/2024-11-05/basic/lifecycle.mdx>
- <https://github.com/modelcontextprotocol/rust-sdk/blob/main/crates/rmcp/src/transport/child_process.rs>
- <https://github.com/modelcontextprotocol/typescript-sdk/blob/main/packages/client/src/client/sse.ts>
- <https://tokio.rs/tokio/topics/shutdown>

## Options Considered

1. Keep best-effort `close -> ()` and add more logs. Rejected because a caller
   still cannot distinguish settled cleanup from an orphan task or process.
2. Add a manager-level cleanup supervisor. Rejected because it creates a
   second resource authority outside the transport that spawned the I/O task or
   child.
3. Keep each transport as the unique resource owner, fence new requests during
   close, settle pending calls, await owned tasks and children within one
   deadline, and propagate a close `Result` through client and manager APIs.
   Accepted.

## Decision

MCP close is an awaited and observable settlement boundary.

- `McpTransport::close`, `McpClient::close`, `McpManager::disconnect`, and
  `McpManager::close_all` return `Result`. Manager close attempts every client
  and returns one aggregate error after all attempts, so one failure cannot
  strand later connections silently.
- A transport fences request admission before draining its pending map.
  Request registration owns a synchronous removal guard, so endpoint failure,
  POST/write/flush failure, response timeout, response-channel close, caller
  cancellation, and explicit transport close all remove the exact request.
- SSE close cancels the connection/reconnect loop, drains all pending requests,
  and awaits its owned task. The stream receive and POST paths observe the same
  cancellation token. If task settlement exceeds the close deadline, the task
  is aborted and awaited, and close returns an error rather than success.
- Stdio stdout EOF and read error both fence admission and fail all pending
  requests. Explicit close serializes with request writes, takes and drops
  child stdin, drains pending requests, waits for the child, escalates to kill
  after the graceful interval, and awaits the stdout and stderr tasks. These
  steps consume one absolute close deadline; timeout, join, kill, and wait debt
  is returned to the caller.
- Stateful SSE and stdio close is idempotent. Concurrent close calls serialize
  on the same transport owner and do not create detached duplicate cleanup. The
  public close Future waits on a shared receipt while an internal single-flight
  task owns cleanup, so caller timeout or cancellation cannot detach the task or
  child owner.
- Manager resource maps retain active clients until close succeeds.
  Failed close becomes retryable cleanup debt under the same authority;
  concurrent manager close calls share one gate, and replacement is not
  published when the previous transport fails to settle.
- `McpClient` and `McpManager` remain the only connection-level lifecycle
  facades. No new registry, process supervisor, product state, or alternate
  terminal authority is introduced.
- MCP servers are user-selected local extensions. This lifecycle repair adds
  no connection permission gate and does not reinterpret Agent automation
  permission modes.

## Consequences

Public close callers must handle a `Result`. Close can take up to the bounded
cleanup interval because successful return now proves that the transport-owned
task and child resources reached a safe point. Unexpected EOF or read failure
rejects new sends and releases every pending caller instead of waiting for each
request timeout.

SSE remains a legacy MCP 2024-11-05 compatibility transport. This decision does
not extend its protocol surface, modify LSP lifecycle behavior, or change MCP
connection policy.

## Verification

Fault-injection tests cover endpoint/POST/timeout cancellation-safe pending
removal, SSE close drain plus task join, stdio EOF/read-error pending failure,
graceful and forced real-child shutdown, bounded close failure, idempotent
close, and manager aggregation that continues after a failing client. Focused
tests, formatting, and `echo_integration` Clippy run before branch handoff; the
workspace-wide merge gate runs only on the integration branch before its MR to
`main`.

## Construction Cancellation Follow-Up (Issue #55)

The original transport decision did not cover cancellation before a client
was published. A preparation waiter could be dropped during initialization,
and its `Drop` handler launched an untracked retry loop. SSE construction also
held a receive task across a warmup await and discarded its join waiter on
drop. Runtime shutdown could stop either cleanup task before settlement.

The considered options were another detached retry loop, a separate global
process registry, and a retained preparation scope under the existing client
and manager lifecycle. The retained scope was chosen: `McpClient::new` returns
a concrete awaitable preparation with a cloneable cleanup scope, and
`McpManager` registers that scope before the first resource-creating poll.
`close_all` cancels and awaits every registered preparation, retaining failed
close as retryable debt. A cancelled scope cannot publish a late initialized
client. Direct consumers that cancel a preparation waiter retain its scope and
await `close`, retrying the same scope after a cleanup error. SSE construction
now transfers its receive task into the transport before suspension; endpoint
discovery remains part of the subsequent MCP handshake.
Manager topology methods retain their exclusive `&mut self` contract, so a
remove cannot overtake an in-progress connection handshake. A caller that
drops a blocked reconcile Future can then await `close_all` on the same manager
to settle its retained preparation scope.

Transport close receipts retain their producer task handle. If its runtime
stops while a receipt still says `Closing`, a later close starts a new
settlement attempt. Stdio keeps the child in its owner slot while awaiting
exit, so an interrupted close cannot lose the child before a retry can reap
it. These rules also cover a manager close interrupted after preparation has
handed off to an installed client.

This follows the same transport-owned, awaited shutdown guidance cited above
from the MCP specification and Rust and TypeScript SDKs. Tokio's graceful
shutdown guidance likewise requires tracking spawned tasks and waiting for
them to finish. No product-specific registry or permission policy is added.
