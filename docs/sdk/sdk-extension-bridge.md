# SDK extension bridge (bidirectional)

The extension bridge lets a host language implement public `echo_agent`
framework traits and have the Rust Agent call them back over the same ACP
connection — with Rust semantics preserved under timeout, cancellation,
disconnect and generation races. It is negotiated as the
`extension_bridge` capability of the `_echo_agent` profile and compiled
only when the Host is built with the `sdk-extension-bridge` feature.

Status: delivered in the Rust Host (source-built, real-process E2E). The
TypeScript/Python/Java SDKs that would consume it do not exist yet — the
program still does not claim **Runnable**.

## Model

```text
framework trait call (Tool / LlmClient / Store / ...)
  -> thin proxy (echo-sdk-host)
  -> lease from the connection's ExtensionInvocationAuthority
  -> one typed _echo_agent/extension/invoke request (Host -> SDK)
  -> SDK dispatcher runs the host-language implementation
  -> result | stream | typed error
  -> proxy restores the Rust trait value; framework state machines continue
```

One connection owns one invocation authority
(`echo_agent::acp::ExtensionInvocationAuthority`): bounded concurrency
permits, per-invocation cancellation, exclusive-mutation leases and
exactly-once settlement. Proxies never touch a second run/session/terminal
authority — `ToolManager` policy, run terminals and receipts stay with the
framework.

## Extension kinds and operations

Each registration names one `ExtensionKind`, a typed per-kind
`ExtensionDescriptor` (versioned; unknown versions fail closed) and a
client-side `implementation_id`. The closed `ExtensionOperation` set binds
every reverse call to its kind; dispatching an operation to the wrong kind
is rejected before any callback leaves the process.

| Kind | Operations | Injection point |
|---|---|---|
| `tool` | `tool_execute`, `tool_execute_stream`, `tool_validate_parameters` | `ReactAgent::add_tool` |
| `llm_client` | `llm_chat`, `llm_chat_stream` | `ReactAgent::set_llm_client` |
| `store` | `store_put/get/search/search_with/delete/list_namespaces/list` (+prune/dedup) | `set_memory_store` (memory tools re-registered against the extension) |
| `human_loop_provider` | `human_loop_request` | `set_approval_provider` + the appeal tool |
| `hook` | `hook_run` | `HookRegistry::set_programmatic_hook` |
| `agent_callback` | `callback_on_*` (observational) | `add_callback` |
| `intervention_callback` | `intervention_on_tool_call/think_start/final_answer` | `add_intervention_callback` |
| `agent_factory` | `factory_create_agent` | `register_subagent_factory` (lazy construction) |
| `custom_agent` | `agent_execute(_stream)/chat(_stream)/close` | `register_agent` (subagent dispatch by name) |

Registrations are **connection-owned**: they never survive a Host restart
or a reconnect, and they take effect for Session Agents constructed after
the registration. Re-registering the same identity with the same
descriptor fingerprint returns the same handle; a different descriptor is
a typed `extension_conflict`.

## Reverse invocation contract

`_echo_agent/extension/invoke` (Host → SDK) carries the extension handle,
a fresh invocation identity (never a JSON-RPC request id), the operation,
an optional session/run correlation context, the typed payload selected by
kind + operation, the deadline and — for streaming operations — the
**Host-minted stream handle** the SDK must echo.

The SDK answers with exactly one of:

- `result` — one typed payload (the trait's return value);
- `stream` — the echoed stream handle, payload delivered through
  `_echo_agent/extension/stream` notifications;
- `error` — a typed `EchoSdkError`.

Failure semantics (no built-in fallbacks, design §12.1):

- **deadline** — the Host settles `extension_timeout` locally, sends the
  `_echo_agent/extension/cancel` notice with reason `timeout`, and discards
  the late answer;
- **cancellation** — framework/run cancellation settles `cancelled` and
  notifies with reason `cancelled`; the framework's terminal stays the
  only one;
- **disconnect** — the official transport reports the closed connection and
  the invocation settles `extension_disconnected`;
- **late response** — an answer after settlement is discarded with bounded
  diagnostics; it can never overwrite settled state;
- **re-entry** — a second exclusive invocation on the same registration
  (human loop, hook, factory, custom agent) fails fast with
  `extension_conflict` instead of waiting (design §12.3).

## Streams

Streaming callbacks deliver `chunk` events with per-stream monotonic
sequences starting at 1 and exactly one terminal (`complete`, `failed` or
`cancelled`). The Host enforces exactly-one-terminal and sequence
monotonicity at a bounded sink; events for unknown or released streams are
discarded with bounded diagnostics. The Rust stream ends exactly at the
terminal and releases the stream handle — a consumer can never observe a
stream that keeps waiting past its terminal.

## Negotiation and admission

- The Host advertises `extension_bridge` plus its bounds
  (`max_registered_extensions`, descriptor/payload/stream byte limits,
  in-flight invocation and callback concurrency, default callback
  timeout) only when the bridge is compiled; a plain standard Client
  receives method-not-found for every `_echo_agent/extension/*` call.
- Every handler walks the fixed ladder: extended-mode gate → capability
  gate → handle shape/kind/generation/closed → descriptor and payload
  bounds → framework work.
- Connection teardown order (design §12.3): close admission → cancel
  in-flight invocations → bounded settlement drain → profile flush →
  Session Agent/MCP close → release extension registrations and handles.

## Secrets and output discipline

Descriptors, events, errors and diagnostics never carry credentials; the
Host's credential stays out of stderr (asserted in the E2E). stdout carries
only the official ACP wire.

## Verification

- `echo-sdk-protocol` contract tests: typed descriptors, operation
  taxonomy, official RPC derives, fixtures and fail-closed samples
  (`tests/extension_contract.rs`, `tests/core_rpc_contract.rs`).
- Shared runtime regression: concurrency permits, exclusive conflicts,
  deadline/cancel settlement, late-response discard, teardown order
  (`tests/acp_extension_runtime.rs`).
- Real-process E2E with the official Client as the SDK dispatcher
  (`echo-sdk-host/tests/extension_bridge_e2e.rs`): tool round trip with
  callbacks, intervention and hooks; streaming LlmClient; registration
  deadline timeout; framework cancellation notice; plain-client
  fail-closed; unregister idempotency and stale-generation ladder.
