# echo-agent SDK core profile (`_echo_agent/*`)

The core profile is the first negotiated extension profile of the
echo-agent SDK Host. On one standard ACP v1 connection it exposes the
framework's Agent / Session / Run object model with typed JSON-RPC methods,
generation-fenced handles, full `EventEnvelope` streaming, bounded live
delivery with ACK, durable replay and restart recovery.

> **Status.** The core profile is delivered as a Rust Host capability with
> real-process E2E coverage (see `echo-sdk-host/tests/core_profile_e2e.rs`).
> No language SDK speaks it yet, so the program is still **not Runnable**
> in the [status ladder](README.md#status-ladder) sense: that requires at
> least one language's full extension path.

## Enabling the profile

The Host serves the core profile only when all of the following hold:

1. The Host binary was built with the `sdk-core-profile` feature:
   `cargo build -p echo-sdk-host --features sdk-core-profile`.
2. The Host configuration carries an explicit `sdk_profile` section with an
   **absolute** `state_root`. The Host never discovers a state root from the
   working directory, home, `.env` or product configuration.
3. The Client publishes a hello under `clientCapabilities._meta.echo_agent`
   during `initialize`, and every field matches the Host advertisement
   published under `agentCapabilities._meta.echo_agent`.

```jsonc
// host configuration (excerpt)
{
  "schema_version": 1,
  "default_agent": { /* model + agent settings, as the standard profile */ },
  "sdk_profile": {
    "state_root": "/var/lib/echo-agent-sdk-host",   // absolute, created on demand
    "limits": {
      "max_frame_bytes": 1048576,          // one newline-delimited stdin frame
      "max_event_bytes": 1048576,          // one serialized live notification
      "max_outstanding_live_events": 128,  // ACK window before one gap + pause
      "max_replay_events": 512,            // per replay request
      "max_replay_bytes": 8388608,         // per replay request
      "max_open_handles": 512,             // open Agent/Session/Run/Stream handles
      "shutdown_timeout_secs": 5
    }
  }
}
```

Without `sdk_profile` the Host is standard-only: it never advertises the
extension and every `_echo_agent/*` request answers with the official
JSON-RPC `method not found` (-32601). A standard ACP Client is unaffected.

## Negotiation

- **Advertisement.** During `initialize` the Host publishes an
  `EchoAgentCapability` under `agentCapabilities._meta.echo_agent`: the
  extension protocol version, the extension `contract_digest` and
  `source_contract_digest` (both sha256 over machine-generated contract
  artifacts), the compiled leaf features, the declared capability families
  and the resource limits.
- **Hello.** An SDK Client publishes an `EchoAgentClientHello` under
  `clientCapabilities._meta.echo_agent` with the same version/digest pair
  plus its required features and capabilities.
- **Decision.** Extended mode is entered only when version, both digests,
  every required feature and every required capability match. Any mismatch
  degrades the connection to Standard **without failing `initialize`** and
  without creating handles — the SDK Client fails closed on its side when it
  does not see an acceptable advertisement.
- All extension data lives inside `_meta`; standard ACP fields are never
  modified (ACP extensibility rules).

## Capabilities delivered

| Family | Capability | Methods |
|---|---|---|
| Agent lifecycle | `agent_lifecycle` | `_echo_agent/agent/create`, `/describe`, `/close` |
| Session handles | `session_handles` | `_echo_agent/session/create`, `/load`, `/close` |
| Runs | `runs` | `_echo_agent/run/start`, `/get`, `/wait`, `/cancel`, `/steer`, plus the `_echo_agent/event`, `_echo_agent/event/ack` notifications |
| Event replay | `event_replay` | `_echo_agent/run/replay`, plus the `_echo_agent/gap` notification |

Task graphs, subagents, extension bridge, structured output and feature
operations are **not** part of this profile yet; they are not advertised and
their methods answer method-not-found.

## Object model and handles

- `agent/create` binds either the Host default definition
  (`{"variant":"host_default"}`) or an explicit versioned config projection
  (model + agent settings, with a mutually exclusive inline/env credential
  source). A handle references the immutable **definition**; every Session
  still constructs its own independent framework Agent.
- `session/create` mints a Session over an agent definition. The response
  also carries the ACP Session id, so the same Session is addressable by
  standard `session/prompt` / `session/cancel` — both entries are the same
  object with one run slot.
- `run/start` begins one chat or execute run and returns a **Run handle**
  plus a **Stream handle**. Runs execute asynchronously; `run/wait` waits
  bounded for the single authoritative terminal.
- Handles are `{id, generation, kind}`. Validation order is fixed:
  shape → kind → generation → issued/closed. A never-issued id is
  `invalid_value`, a pre-restart generation is `stale_handle`, a released
  id is `closed_handle`. Handles never rebind.
- `agent/create` with an `idempotency_id` returns the same handle for the
  same canonical config and a typed conflict for a different one.

## Standard ↔ extension bridging

In Extended mode the standard responses carry the extension handles too:
`session/new` responses get `_meta.echo_agent.session`, and prompt turns get
`_meta.echo_agent.run` + `stream`. A standard Prompt and an extension
`run/start` share the same run ids, the same cancellation authority and the
same event ledger — `_echo_agent/event` notifications are also emitted for
prompt turns in Extended mode.

## Events, ACK and replay

- Every accepted framework `EventEnvelope` (identity, sequence, content
  hash, parent link preserved verbatim) is committed to the run's durable
  journal first, then delivered as the standard `session/update` projection
  and as a `_echo_agent/event` notification. Exactly one terminal event ends
  a run; terminals come only from the framework.
- The Client acknowledges consumed cursors with `_echo_agent/event/ack`.
- While the outstanding (un-ACKed) count/bytes stay inside
  `max_outstanding_live_events` / `max_event_bytes`, delivery is live. At
  the bound the Host sends **one** `_echo_agent/gap` notification and pauses
  live delivery. The Client recovers with `run/replay` from its cursor and
  then ACKs; live delivery resumes after the acknowledged cursor. Host
  memory stays bounded regardless of consumer speed.
- `run/replay` is bounded by `max_replay_events` / `max_replay_bytes` and
  addressed by a current-generation stream handle. Replays below the journal
  retention floor return a typed gap with a snapshot watermark. A replay
  response is validated against the contract before it is sent.

## Persistence, recovery and shutdown

The state root layout:

```text
<state_root>/runtime_state/     framework Agent checkpoints (FileRuntimeStateStore)
<state_root>/journals/<run>/    one segmented journal per run (full events)
<state_root>/host/generation    monotonic Host generation counter
<state_root>/host/session-index.json  Session identity, Agent binding and lossless cwd
<state_root>/host/run-index.json  run identity/status/terminal records
```

- Each start advances the Host generation; the process-exclusive state-store
  lease prevents two live Hosts on one root.
- `session/load` re-binds a persisted session id at the **current**
  generation: fresh Session/Run/Stream handles, historical runs reported
  from the index, journals replayable. The framework resumes the Agent from
  its committed checkpoint; completed tool facts are not re-executed.
- A run that was active when the previous process died is recovered as
  `status: "interrupted"`: `run/get` shows no terminal and no receipt,
  `run/wait` answers the typed `host_exited` error. The Host never revives
  the interrupted driver and never fabricates a completed terminal. New runs
  continue from the framework's committed checkpoint only.
- Shutdown order (stdin EOF and clean close share it): stop run admission →
  cancel active runs → bounded wait for receipts → flush journals/index →
  close Session Agents → release handles.

## Errors

`_echo_agent/*` failures use one fixed JSON-RPC server-error code
(`-32050`); `error.data` carries the bounded `EchoSdkError` envelope
(`code`, `message`, `retryable`, optional `operation`/`handle`/`details`).
Malformed JSON, bad params and unknown methods keep returning the standard
JSON-RPC errors from the official runtime. The closed error-code list and
bounds live in `echo-sdk-protocol` (`error.rs`) and the generated schema
(`contracts/sdk/schema/echo-agent-extension-v1.schema.json`).

## Transport discipline

stdin frames are byte-counted before the official JSON-RPC parser sees them;
a frame larger than `max_frame_bytes` fails the connection with a bounded
stderr diagnostic and no business side effect. stdout remains the single
official ACP writer; diagnostics go to stderr and never contain credentials.
