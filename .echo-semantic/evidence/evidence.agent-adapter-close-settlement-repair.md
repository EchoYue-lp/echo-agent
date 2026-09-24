---
schema_version: 1
id: evidence.agent-adapter-close-settlement-repair
kind: evidence
observed_at: source:d0d70f0819595200d148564ce11cb55de583ba943d7a5554b4c06a0f6dcb8af9
source_refs:
  - src/acp/session.rs
  - src/acp/runtime.rs
  - src/acp/adapter.rs
  - src/acp/mod.rs
  - echo-orchestration/src/runtime/turn_driver.rs
  - src/headless.rs
  - src/lib.rs
  - echo-integration/src/channels/manager.rs
  - echo-integration/src/channels/session.rs
  - echo-integration/src/channels/types.rs
  - src/channels.rs
  - src/agent/react/mod.rs
  - docs/adr/0066-agent-adapter-close-ownership.md
supports: [behavior.agent-turn-lifecycle, behavior.protocol-projection, rule.turn-terminal-authority]
limitations:
  - ACP requires a caller-retained AcpAdapterCloseHandle; consumers without one fail before Agent creation
  - A2A remains untouched and owned by its separate Findings
  - Agent implementations that ignore cancellation can exceed one close attempt and require the caller to retain the owner
---

# Agent adapter close settlement repair

## 支持的结论

## Owner and ordering

ACP uses its existing `SessionRegistry` close lease and framework `RunEntry`
receipt. The registry permanently fences creation, including factory work
already in progress. Connection close cancels extension invocations and Runs
concurrently, retains a Run without its framework receipt, waits for profile
settlement, and attempts Agent close after callback/Run drain even if profile
flush fails. A failed Session Agent close retains the original registry entry;
transport close failure triggers one EOF fallback retry. A mandatory
`AcpAdapterCloseHandle` retains the exact same services/profile owner after
both attempts fail; an unretained adapter cannot begin the connection or
create an Agent. `connect_retaining_close_owner` synchronously returns the
handle before its connection future is polled. Manual official Client
callers must keep their handle until cleanup succeeds. No adapter-generated
Turn terminal is introduced.

Headless synchronously publishes `HeadlessRunHandle` before its owned task runs.
Dropping or aborting a `run_headless` waiter requests Turn cancellation while
the task retains the Agent through close and publishes one result receipt. A
failed close remains owned by the handle for `retry_close`. Missing runtime
startup and runtime shutdown before first poll also return a failure receipt
without dropping the close owner. A caller token is observed through a child
scope, and `retry_close` rejects calls before the result receipt exists.

ReactAgent has one close authority that atomically fences admission, owns the
child cancellation token for every accepted direct or streaming Turn, and
waits for the Turn's existing terminal path before MCP cleanup. Preparation
becomes settlement-bearing before reconcile, hydrate, guard, audit, trace, or
context awaits. Normal pre-turn rejection releases the lease explicitly;
caller abort, producer abort, or panic records persistent close debt and blocks
MCP cleanup. A cancelled close waiter does not reopen admission or discard
leases; a later close waits or reports the same debt. The existing
`AgentTurnDriver` classifies a typed cancellation before stream creation as
`TurnOutcome::Cancelled`, preserving the same terminal kind at every phase.

ChannelManager records transport-stopped and handler-close as separate phases,
then retains handlers until both succeed. A retry after transport success goes
directly to the same handler even when `ChannelPlugin::stop` is non-idempotent.
SessionHandler fences session creation, cancels
sender generations, waits for every accepted stream and delivery lease during
shutdown, and closes each inner Agent before removing it or publishing a
timeout/reset replacement. Legacy/custom stream receipts are resource lifetime
only; reset keeps its existing driven-only terminal settlement. Failed or
cancelled close leaves the same handler/session owner. `ReactAgent::drop` no
longer spawns unawaited MCP cleanup; adapters await the existing Agent close.

## 来源与范围

The sources above cover ACP, Headless, Channel and ReactAgent close owners.
A2A remains outside this repair candidate.

## Decision authority

ADR 0066 records alternatives, industry patterns, layer ownership, bounds,
and one-shot limitations. ADR 0046 remains Turn execution/delivery authority;
ADR 0057 remains Channel generation-delivery authority; ADR 0049 remains MCP
transport/client cleanup authority.

## 已知缺口

ACP requires external consumers to retain its close handle before `ConnectTo`
consumes the adapter. Dropping a manual handle after admission forfeits retry
ownership and violates this contract. A2A remains outside this Evidence.
Integration gates, independent rereview, and mainline delivery are separate
obligations.
