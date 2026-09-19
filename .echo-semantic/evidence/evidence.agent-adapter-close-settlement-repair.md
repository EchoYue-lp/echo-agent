---
schema_version: 1
id: evidence.agent-adapter-close-settlement-repair
kind: evidence
observed_at: source:16e4824bc89e28e36a6c329505451b8ca5c86d6e4f4d1144ea01616535f3ac09
source_refs:
  - src/acp/session.rs
  - src/acp/runtime.rs
  - src/acp/adapter.rs
  - src/acp/mod.rs
  - src/headless.rs
  - echo-integration/src/channels/manager.rs
  - echo-integration/src/channels/session.rs
  - echo-integration/src/channels/types.rs
  - src/channels.rs
  - src/agent/react/mod.rs
  - docs/adr/0066-agent-adapter-close-ownership.md
supports: [behavior.agent-turn-lifecycle, behavior.protocol-projection, rule.turn-terminal-authority]
limitations:
  - Headless reports close errors but cannot return a retry owner after its one-shot call
  - Dropping or aborting the outer run_headless future before its close phase cannot guarantee awaited cleanup
  - ACP requires a caller-retained AcpAdapterCloseHandle; consumers without one fail before Agent creation
  - A2A remains untouched; its close, terminal authority and stream cleanup findings remain open
  - Agent implementations that ignore cancellation can exceed one close attempt and require the caller to retain the owner
---

# Agent adapter close settlement repair candidate

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

Headless awaits its driven Turn receipt then `Agent::close` and marks a close
error unsuccessful. A2A is untouched and none of its close, terminal or stream
lifecycle Findings are claimed by this evidence.

ChannelManager retains handlers from successful start until transport stop
and handler close both succeed. SessionHandler fences session creation, cancels
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

Headless cannot return a retry owner. ACP now requires
external consumers (including the independent SDK Host) to retain its close
handle before `ConnectTo` consumes the adapter. Dropping a manual handle after
admission forfeits retry ownership and violates this contract. A2A remains
open and untouched. Integration gates, independent rereview, and mainline
delivery are separate obligations.
