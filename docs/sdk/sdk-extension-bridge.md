# SDK extension bridge (bidirectional)

The extension bridge lets a host language implement public `echo_agent`
framework traits and have the Rust Agent call them back over the same ACP
connection — with Rust semantics preserved under timeout, cancellation,
disconnect and generation races. It is negotiated as the
`extension_bridge` capability of the `_echo_agent` profile and compiled
only when the Host is built with the `sdk-extension-bridge` feature.
That feature also enables `sdk-facade-adapters`, because compressor callbacks
invoke their temporary Host tokenizer through the canonical facade route;
feature-surface advertisement therefore reflects both compiled handlers.

Status: delivered in the Rust Host and all three source-built language SDKs.
TypeScript, Python and Java expose the same registration, callback,
cancellation and stream boundaries; the full intrinsic/all-feature parity
matrix remains open.

Build the Host from source with `cargo build -p echo-sdk-host
--features sdk-extension-bridge --locked`; this feature includes the core
and facade-adapter profiles. The Host does not bundle a language runtime or a
prebuilt artifact.

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
| `channel_plugin` | `channel_start/stop/send/health` | `channels` facade manager (when `framework-channels` + `sdk-extension-bridge` are compiled) |
| `channel_message_handler` | `channel_handle`, `channel_handle_stream`, `channel_reply` | ChannelPlugin/MessageHandler reverse adapter |
| `context_compressor` | `compressor_compress` | `ReactAgent::set_compressor` for Sessions constructed after registration |
| `agent_component` | closed component operation enum plus component stream | ConversationStore, RunStore, RuntimeStateStore, AuditLogger, PreModelContextProjector, MemoryTriggerSink, Guard, SearchProvider, Workflow CheckpointStore, RevisionedTaskStore, SandboxExecutor, McpTransport, Embedder, MemoryPromoter, Workflow, IntentClassifier and SkillLoadPolicy; injected into new Sessions or consumed by an explicit resource operation |

Registrations are **connection-owned**: they never survive a Host restart
or a reconnect, and they take effect for Session Agents constructed after
the registration. Re-registering the same identity with the same
registration fingerprint (descriptor plus default timeout) returns the same
handle; a different snapshot is a typed `extension_conflict`.

Logical runtime identities are unique within a connection: Tool and directly
registered CustomAgent names may have only one owner, and
Store/HumanLoopProvider are singletons. LlmClient registrations are the
deliberate exception; the most recent registration is selected by monotonic
registration order. Opaque handle UUID order never chooses an implementation.

## Reverse invocation contract

`_echo_agent/extension/invoke` (Host → SDK) carries the extension handle,
a fresh invocation identity (never a JSON-RPC request id), a named
`invocation` union containing the operation and its typed input, an optional
session/run correlation context, the deadline and — for streaming operations
— the **Host-minted stream handle** the SDK must echo.

The protocol crate exposes the operation-discriminated `ExtensionInvocation`
and `ExtensionResult` unions, plus public DTOs for each extension kind. The
generated schema is the source consumed by future TypeScript, Python and Java
dispatchers; `WireValue` remains only at explicitly open leaf fields such as
user parameters, metadata and provider-specific raw values.

An LlmClient descriptor may set `supports_streaming: false`. The Host then
invokes `llm_chat` and adapts the single typed response into the framework's
one-terminal chunk stream; it never calls an operation the implementation
declared unsupported. The response must contain a non-empty, bounded
`finish_reason`; the Host rejects an ambiguous response instead of inventing
`stop` or `tool_calls`.

A ContextCompressor call carries the framework message set, lossless token
limit, current query, focus instructions and a temporary Host-owned tokenizer
resource. The three SDKs expose `countTokens` / `count_tokens` against that
resource, so a compressor uses the exact calibrated tokenizer selected by the
ContextManager instead of a language heuristic. The resource is owner-checked
and released when the callback settles. The SDK returns retained and evicted
messages plus an optional compression checkpoint; cancellation, timeout and
disconnect settlement remain Host-owned.
Its descriptor carries the stable strategy name used by Rust compression
metrics. `ReactAgent::set_compressor` accepts an extension handle for an
explicit live-Session replacement; different Sessions can bind different
registrations.

Agent infrastructure components share one typed bridge family but not one
untyped callback. The descriptor has a closed component discriminator;
`AgentComponentCallInputWire` and `AgentComponentCallResultWire` are
operation-discriminated unions that freeze every argument/result shape and
lossless integer field. Rust proxies implement the actual framework traits and
validate the returned component and operation before restoring the value.
TypeScript exposes the same discriminated unions, Python validates the closed
operation set and named argument map, and Java decodes calls into the sealed
`AgentComponentRequest` hierarchy.

Defaultable Rust trait methods are still explicit bridge operations when an
implementation may override them: ConversationStore ensure/search, RunStore
append/parent-list, Sandbox cancel-aware execution and Workflow/Sandbox streams
all cross the callback boundary. `supports_streaming` is valid only for
SandboxExecutor and Workflow. Their stream items use the Host-issued extension
stream handle and separate typed Sandbox/Workflow chunk and terminal unions.
Sandbox `complete/failed` and Workflow `completed` can only be sent as the
outer stream terminal; output/node/token events can only be chunks. The Host
then republishes Workflow extension events through the same canonical
`WorkflowEvent` projection as graph streams. No buffered result is presented
as a live stream. IntentClassifier receives the exact user input and message
context and can be installed with an explicit IntentRouter config and
available-skill fence. SkillLoadPolicy receives every public descriptor field
through `skill_load_allows`; discovery, prepared-plugin registration and
reconciliation await that same callback on the live Session Agent.

WorkflowCheckpointStore does not inherit a successful no-op for claim
settlement. The bridge transports generation-CAS save plus attempt-fenced
renew, acknowledge and requeue calls. Its descriptor declares a canonical
`claim_heartbeat_interval_ms` from 1 through 300000, and the Host schedules
renewal from that negotiated capability. Missing operations, owner mismatch or
unsupported settlement fail before a continuation can be reported as settled.

Cancel-aware Sandbox callbacks preserve the framework error domain: extension
cancel and timeout settle as `SandboxError::Cancelled` / `SandboxError::Timeout`,
and Run cancellation waits for the component cleanup call before publishing
the cancelled terminal.

AgentComponent admission is operation-scoped. Workflow run/run-stream calls
are exclusive because the Rust trait takes `&mut self`; Send+Sync `&self`
components such as Store, AuditLogger, Guard, SearchProvider and Embedder may
serve multiple Sessions concurrently. Same-Session callback mutation remains
an independent `extension_conflict` rule.

MCP transport notifications start polling only when `notification_rx()` is
requested, use a bounded 64-item oldest-drop queue, and stop on transport
close. MCP initialization failure closes the supplied transport. A successful
`McpClient::from_transport` initialization whose facade handle cannot be
published also closes the client before returning the quota error. Manager
connect plus client/tool handle publication is transactional: any tool-handle
quota failure removes the client handle and disconnects the manager.

No public `extension` item is relabeled as a same-topic family/core operation.
Traits with a live Host consumption point use a typed proxy. Rust generic,
borrowed-view, `FnOnce`, marker/builder, event-bus registration and
runtime-owned construction traits without a Host call site carry an exact
process-local evidence route and remain language interface/helper obligations.
A frozen intrinsic-membership digest makes any new intrinsic item an explicit
contract change.

AgentFactory results are invocation-scoped instances rather than direct
CustomAgent registrations. Repeated or concurrent factory results may use the
same logical name; each instance receives its own handle and is sent
`agent_close` before its dispatch terminal is released. A drop/cancellation
fallback inside the Host runtime schedules the same close and releases the
handle; outside a Tokio runtime it still releases the handle fail-closed but
cannot send an asynchronous request. Factory instances are excluded from the
Session-visible registration view, so a concurrently created Session cannot
capture another Session's temporary agent.

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
- **re-entry** — a second exclusive invocation on the same registration, or an
  extension callback attempting an exclusive mutation of its active Session,
  fails fast with `extension_conflict` instead of waiting on the Agent lock
  (design §12.3).

## Streams

Streaming callbacks deliver `chunk` events with per-stream contiguous
sequences starting at 1 and exactly one terminal (`complete`, `failed` or
`cancelled`). The Host enforces exactly-one-terminal and sequence
continuity at a bounded sink. A wrong payload kind, gap, duplicate before
settlement or mailbox overflow settles a typed failed terminal immediately;
events for unknown or already-released streams are discarded with bounded
diagnostics. The Rust stream ends exactly at the terminal and releases the
stream handle — a consumer can never observe a stream that keeps waiting
past its terminal. The callback mailbox capacity uses the negotiated
`max_stream_buffer_events` / Host `max_outstanding_live_events` bound, so the
advertised limit and runtime allocation cannot drift.

The contract uses different named unions for chunks and completion. Tool
chunks can only be progress/output while Tool completion carries a result;
LLM chunks cannot carry a finish reason while completion requires one; and
CustomAgent chunks contain only non-terminal Agent events while completion
contains only final answer, error or cancellation.

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

## Relation to the facade feature adapters

The facade feature adapters
([facade-feature-adapters.md](facade-feature-adapters.md)) route the
feature-family surfaces to the framework's own services; the bridge remains
the only reverse-call path for consumer-implemented traits. A facade family
never simulates a result when a bridge callback fails — typed errors
propagate unchanged.

## Verification

- `echo-sdk-protocol` contract tests: typed descriptors, operation
  taxonomy, official RPC derives, fixtures and fail-closed samples
  (`tests/extension_contract.rs`, `tests/core_rpc_contract.rs`).
- Shared runtime regression: concurrency permits, exclusive conflicts,
  deadline/cancel settlement, late-response discard, teardown order
  (`tests/acp_extension_runtime.rs`).
- Real-process E2E with the official Client as the SDK dispatcher
  (`echo-sdk-host/tests/extension_bridge_e2e.rs`): tool round trip with
  callbacks, intervention and hooks; Store/HITL; AgentFactory/CustomAgent;
  streaming and non-streaming LlmClient; ContextCompressor injection,
  late binding and compression; Agent component consumption; channel plugin lifecycle/send;
  malformed kind, oversized chunk and
  out-of-order immediate failure; duplicate terminal; real Host mailbox
  backpressure; registration deadline with late response; framework
  cancellation/consumer-drop notice; SDK disconnect; plain-client
  fail-closed; semantic identity conflicts, unregister idempotency and the
  stale-generation ladder.
- Host bridge unit tests cover complete typed AgentEvent round trips,
  duplicate sequence settlement and bounded-mailbox failure retention.
