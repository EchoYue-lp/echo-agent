# SDK protocol contract

This document describes the wire-level contract of the echo-agent SDK: the
two profiles, the extension namespace, lossless scalar rules, error taxonomy,
event/replay semantics and versioning. The stable initialize/new/prompt/update/
cancel subset now has a transport-neutral Rust Agent adapter; the extension
profile, source-built SDK Host and language SDKs remain later deliveries (see
the [status ladder](README.md#status-ladder)).

## Base protocol: official ACP v1

The SDK builds on the stable [ACP v1](https://agentclientprotocol.com/protocol/v1/extensibility)
wire protocol. Everything standard is owned by the official artifacts pinned
in [`contracts/sdk/acp-baseline.json`](../../contracts/sdk/acp-baseline.json):

| Layer | Version (pinned) |
|---|---|
| ACP wire `protocolVersion` | `1` (latest stable; draft v2 explicitly excluded) |
| `agent-client-protocol` crate | `2.1.0` |
| `agent-client-protocol-schema` crate | `=1.7.0` |

The three layers are governed independently: none of them may be inferred
from another (design §18). Tests assert both the lockfile match and that the
official crate's `ProtocolVersion::LATEST` is still `V1` — upstream
promoting the draft would fail the gate loudly.

This repository **never** re-declares JSON-RPC envelopes, `initialize`,
Session, Prompt, ContentBlock, update or stop-reason types. The generated
extension schema is validated to contain none of them.

## Implemented standard Agent adapter

The root `acp` feature exposes `echo_agent::acp::AcpAgentAdapter`. It implements
the official Rust SDK's Agent-side `ConnectTo<Client>` boundary and can attach
to any official transport. Its handlers currently implement stable v1
`initialize`, `session/new`, `session/prompt`, `session/update` and
`session/cancel`; the official runtime also supplies request-level
`$/cancel_request` dispatch.

Each `session/new` calls an `AcpSessionFactory` with the exact cwd, additional
directories, MCP declarations, request metadata and initialized Client
capabilities. The returned framework Agent exclusively owns that Session's
conversation history. The adapter then drives every Prompt through
`AgentTurnDriver` and turns accepted `EventEnvelope` values into bounded ACP
message/thought/tool updates. Both cancellation routes cancel the same
framework token, and only `TurnReceipt` decides the final stop reason or error.

The adapter advertises only the stable baseline it implements. It does not yet
advertise `_meta.echo_agent`, `session/load`, optional lifecycle/config methods,
or rich Prompt content. Text and ResourceLink are accepted; ResourceLink maps
to the provider-neutral structured `LinkedResource` content part with every ACP
field preserved. Text-only Agents fail a ResourceLink Prompt explicitly rather
than receiving an ambiguous private text marker. Other content types fail
before Agent execution. See
[acp-agent-adapter.md](acp-agent-adapter.md) for construction and limitations.

## Two profiles

| | Standard ACP profile | echo-agent SDK profile |
|---|---|---|
| Consumer | any ACP v1 client | echo-agent SDK (TS/Python/Java, future) |
| Methods | standard ACP only | standard + negotiated `_echo_agent/*` |
| Event view | ACP `session/update` (bounded projection) | full `EventEnvelope` extension stream |
| Negotiation | plain `initialize` | `initialize` + `_meta` capability check |

A future standard client ignores the `_meta` capability and keeps working. An
SDK client **fails closed** when the extension protocol version, contract
digest, required capability or feature set does not match — it never
silently degrades to partial parity (design §10.2).

## Extension namespace

All custom methods live under `_echo_agent/*` (leading underscore per ACP
extensibility) and every family is declared in the capability object
published under `initialize._meta.echo_agent`. The frozen catalog (method
name, direction, capability, request, result and error schema) is embedded in
the generated
[`echo-agent-extension-v1.schema.json`](../../contracts/sdk/schema/echo-agent-extension-v1.schema.json)
and enforced by `echo_sdk_protocol::catalog`:

- `_echo_agent/agent/*` — construction, description, close
- `_echo_agent/session/*` — extension session handles
- `_echo_agent/run/*` — start/get/wait/cancel/steer
- `_echo_agent/run/replay` + `_echo_agent/event` + `_echo_agent/gap` —
  lossless event stream, bounded replay, retention gaps
- `_echo_agent/task/*` — TaskRun/PlanTask graph operations
- `_echo_agent/subagent/*` — dispatch/await/control
- `_echo_agent/extension/*` — host-language extension registration and reverse
  invocation (Host → SDK); when an SDK callback returns a stream handle, its
  independently identified chunk/terminal events flow SDK → Host
- `_echo_agent/facade/invoke` and `_echo_agent/memory|workflow|state/op` —
  manifest-identified operations using the closed tagged `WireValue` algebra

The catalog contains **no** standard ACP method and nothing outside the
namespace; both are machine-checked.

The parity manifest does not infer ACP projection from words such as
`prompt` or `session`. Only explicitly listed ACP-owned value families receive
`standard_projection`; builders, trait implementations and process-local Rust
fields are language-intrinsic, while long-lived resources use handles. APIs
visible only under feature combinations record an `all_of` condition rather
than an unexplained `full` marker.

## Identity and handles

JSON-RPC request ids follow the official ACP schema and are never domain
identity. Framework objects cross the wire as
[`WireHandle`](../../echo-sdk-protocol/src/handle.rs): a non-empty domain id,
a generation counter and a typed kind (agent, session, run, stream, task_run,
plan_task, subagent, extension). A handle whose generation no longer matches
resolves to a typed `stale_handle`/`closed_handle` error — never a silent
rebind.

## Lossless scalars

Standard ACP paths are absolute UTF-8 strings and standard numbers must
survive every client runtime. The extension profile therefore carries the
facts ACP cannot (design §10.5):

- `WireI64` / `WireU64` — canonical decimal strings with no JSON precision loss
- `WireDuration` — full-range unsigned seconds plus sub-second nanoseconds
- `WireTimestamp` — signed Unix seconds plus sub-second nanoseconds, including
  times before the epoch (RFC 3339 display is optional)
- `WirePath` — Unix bytes (base64) / Windows UTF-16 units / exact UTF-8
- `WireBytes` — base64 binary
- `WireValue` — a closed tagged algebra for scalar, collection, record,
  variant, handle and unknown additive values; method contracts have no
  schema-free JSON payload escape hatch

All are covered by golden fixtures with mandatory lossless round-trips.
Native path and binary fields declare `echo-*` JSON Schema formats. The
contract validator registers those formats with the same canonical no-pad
base64 and absolute-path functions used by Rust runtime validation, preventing
language generators from accepting a relative encoded path that the Host
would later reject.

## Error contract

Standard methods return standard ACP/JSON-RPC errors. `_echo_agent/*`
methods use the typed envelope in
[`error.rs`](../../echo-sdk-protocol/src/error.rs): stable `code`
(closed set), message, `retryable` classification, optional operation and
handle identity, and bounded details (no raw payloads, no secrets). Codes
cover capability/version/digest mismatch, invalid input, feature
unavailability, stale/closed handles, framework errors, extension bridge
failures (rejected/failed/timeout/disconnected), cancellation, host
shutdown/exit, event gap/replay unavailability and payload/serialization
bound violations.

## Events, replay and gaps

The framework `EventEnvelope` is the event authority; the extension
notification carries every identity fact (schema version, event id, content
hash, sequence from 1, parent link, timestamp) plus the real framework
`AgentEvent` tag/data payload. Contract tests convert an actual framework
event envelope to the wire DTO and back.
Replay is cursor-based (`after_sequence`, bounded `max_events`), and falling
below the retention floor produces a typed `event_gap` with a snapshot
watermark — events are incremental facts, never a substitute for a snapshot
(design §11.2). A run has exactly one authoritative terminal; EOF or process
exit is never success.

## Versioning and compatibility

- Git revision is the source-delivery compatibility boundary.
- The extension protocol version (currently `1`), the contract digest
  (sha256 over the canonical schema document) and the official ACP artifact
  versions move independently.
- Additive wire fields are forward-compatible; unknown values surface as
  `WireValue::Unknown` without crashing older SDKs.
- Removing fields, changing defaults or terminal/cancel semantics, or
  reusing an error code is a breaking change: in this development-phase
  repository such a change updates Host, SDKs, fixtures, manifest and docs
  in the same commit — no legacy fallback is kept.
