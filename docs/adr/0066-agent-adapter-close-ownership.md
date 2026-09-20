# ADR 0066: Settle Agent Resources at Adapter Close

## Status

Accepted

- Date: 2026-09-19
- Owners: ACP, Headless, Channels, and `ReactAgent`

## Context

`echo_core::Agent::close` already returns an awaited, fallible resource-close
future. ACP Session close uses it, but the connection teardown could abandon
Run receipts or stop before Agent close when profile cleanup failed. Headless
returned its Turn result without closing its Agent. ChannelManager stopped
transports without retaining or closing their handlers, and `ReactAgent::drop`
spawned unawaited MCP cleanup. A2A remains outside this decision and keeps its
existing open terminal/stream/close Findings.

Turn execution and delivery already have one framework authority under
[ADR 0046](./0046-turn-execution-delivery-settlement.md). Channel generation
delivery remains owned by [ADR 0057](./0057-channel-generation-delivery-fence.md).
Closing resources must not create another Turn terminal or infer success from
EOF, cancellation, a transport stop, or a dropped future.

Tokio's [graceful shutdown](https://tokio.rs/tokio/topics/shutdown) separates
the cancellation signal from waiting for owned tasks. Codex's
[`SessionRuntime`](https://github.com/openai/codex/blob/main/codex-rs/code-mode-runtime/src/session_runtime/mod.rs)
uses a session cancellation token and tracked task wait; the
[Claude Agent SDK query close](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/query.py)
also awaits its read/transport teardown and makes interrupted cleanup visible.
The shared principle is to fence admission, request cancellation, retain the
owner until work settles, and await the resource close result.

## Options Considered

1. Let each adapter drop its Agent and rely on a spawned cleanup task. This
   cannot report completion or a retryable error to the owner.
2. Introduce a framework-wide close state machine or another Turn terminal.
   This duplicates existing Session, Run, transport, and receipt authorities.
3. Extend each existing adapter owner with admission, cancellation, drain, and
   awaited `Agent::close`, retaining its own failed resources. Chosen.

## Decision

- ACP `SessionRegistry` permanently fences creation. A factory that races the
  fence leaves its completed Agent as a closed registry entry for the same
  close pass. The connection cancels extension invocations and Runs together,
  retains Runs without framework receipts, waits for profile settlement, and
  attempts Session Agent close even if profile settlement or flush fails.
  It skips Agent close while callbacks or Runs remain unsettled. Transport
  close failure permits one EOF fallback retry. Before moving an adapter into
  the official `ConnectTo` trait, callers must retain its
  `AcpAdapterCloseHandle`; otherwise connection setup fails before Session or
  Agent creation. The handle references the same connection services and
  profile, so it can retry after both transport and EOF attempts fail. The
  direct `connect_retaining_close_owner` entry synchronously returns the
  handle before its separately returned connection future can be polled.
  Official `Client::connect_with` callers retain the handle manually until cleanup
  succeeds; dropping it after admission forfeits retry ownership and violates
  this API contract. An early drop before connection starts fails closed. The
  framework receipt remains the only driven Turn terminal.
- Headless starts an owned task and synchronously exposes `HeadlessRunHandle`
  through `start_headless`. The handle owns cancellation, result observation,
  and retry of the same Agent after a failed close. The `run_headless`
  convenience wrapper awaits that handle; dropping or aborting the wrapper
  requests Turn cancellation while the owned task continues through
  `Agent::close` and publishes its receipt. A close error makes the result
  unsuccessful and retains the Agent on the handle for `retry_close`.
  Headless derives a child from a caller-provided cancellation token, so
  wrapper cancellation cannot cancel the caller's parent scope or siblings.
  `retry_close` is valid only after the result receipt is published; once
  close succeeds it is idempotent.
- ChannelManager retains each started `MessageHandler`. A successful transport
  stop precedes awaited handler close, and an interrupted or failed close
  leaves the same handler owned for retry. `SessionHandler` fences creation,
  cancels sender generations, waits for all accepted/polled streams and
  delivery leases, and closes each sender Agent before removing it. This
  shutdown wait includes legacy/custom handlers that do not publish a driven
  terminal; their stream receipt proves resource lifetime only and does not
  create another Turn terminal. Reset keeps its existing driven-only terminal
  settlement policy. Reset and timeout replacement close
  their previous handler before publishing a replacement. Custom stateless
  handlers inherit a no-op `MessageHandler::close`; resourceful handlers must
  override it. `AgentChannelHandler` delegates to its `Agent::close`.
- `ReactAgent::close` permanently fences Turn admission, cancels every active
  or queued Turn, waits for their existing terminal paths to release close
  leases, and only then closes MCP resources. Each admission derives a child
  cancellation token, so closing one Agent does not cancel the caller's parent
  scope. Once preparation may reconcile, hydrate, audit, trace, or mutate
  context, the close lease requires explicit settlement. Panic, forced abort,
  or caller cancellation that drops this path records persistent close debt;
  close returns that error and does not proceed to MCP cleanup. Cancelling a
  close waiter leaves the fence, Turn cancellation, leases, and debt in force;
  a later close waits on or reports the same state. If cancellation is observed
  before a driven stream is created, `AgentTurnDriver` preserves its typed
  `Cancelled` terminal instead of wrapping it as `Failed`. `ReactAgent::drop` does not spawn
  cleanup. It warns when live MCP server names remain; owners must await
  `Agent::close` while they still hold the Agent. MCP transport/client/manager settlement remains under
  [ADR 0049](./0049-mcp-transport-close-settlement.md).

## Consequences

Adapter close is a resource fact separate from the Agent Turn terminal and
delivery outcome. A failed close does not reopen admission. Callers must retain
long-lived owners for retry and should not infer settled resources from a
timeout. `Agent::close` errors are now surfaced by Headless, Channel and ACP
adapter owners. `HeadlessRunHandle` retains a failed Agent close for retry.
ACP's mandatory
close handle is a breaking lifecycle contract for framework and independent
SDK consumers: direct transport callers can use
`let (close_owner, connection) = adapter.connect_retaining_close_owner(transport);`
and then await or spawn `connection`;
official `Client::connect_with(adapter, ...)` callers must retain
`let close_owner = adapter.close_owner()` before moving the adapter, keep it
until close succeeds, and retry `close_owner.close().await` after a failed
connection. The handle adds no
second Session, Run, or Turn authority; it only retains the original owner.

## Verification

Focused tests exercise Headless close failure, ACP creation fencing and
retained receipts, profile error followed by Agent close, EOF retry, mandatory
handle rejection before resource creation, connection-future cancellation,
post-return third close, Channel start/stop cancellation,
transport-before-handler close, legacy stream drop-before-handler-close,
sender session retry, cancelled close, ReactAgent active/queued Turn close,
cancelled close retry, child-token isolation, early guard-future abort,
forced producer abort, provider failure settlement, Headless waiter
cancellation, pre-poll runtime shutdown, start-time cancellation
classification, retry phase validation, and retained Headless close retry.
A2A remains unverified and outside this ADR. Full workspace and independent review are required before
the corresponding semantic Finding can be resolved on main.
