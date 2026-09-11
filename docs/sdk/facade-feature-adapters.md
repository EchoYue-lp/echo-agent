# Facade Feature Adapters (Rust Host)

This document specifies the delivered `sdk-facade-adapters` layer: the
canonical route catalog, the feature model, the resource/stream lifecycle
and the error boundaries of the `_echo_agent/*` facade family surfaces
(plans 07 and 08 of the [SDK design](../supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md)).

> **Status: Rust Host facade parity complete; language route baseline delivered.** Source-built
> TS/Python/Java clients consume the ACP core and generic facade
> invoke surfaces. The Host serves every canonical source operation through a
> concrete adapter or an evidence-backed language-local boundary, plus the task/subagent/structured-output families and
> the stateful (memory/workflow/state/delivery/trace/eval/improve),
> integration (MCP/A2A/LSP/topology) and tool families over the framework's
> own authorities. Consumer traits have a typed bridge or explicit
> process-local evidence, and all public stream routes have
> a concrete lifecycle. Every executable catalog route is covered by the
> language contract suites; process-local intrinsic items remain explicit
> follow-up mappings. The program does not yet claim **Runnable** or **Parity
> complete**.

## 1. Canonical route catalog

The generated `contracts/sdk/facade-operation-catalog.json` is the single
executable route authority. The Host embeds the committed artifact verbatim
(`echo-sdk-host/src/core_profile/facade/registry.rs`) — the same bytes the
contract drift gate verifies — so runtime admission never guesses execution
semantics from paths.

- Every root facade item resolves to **exactly one** canonical route by its
  canonical *source identity*; re-export aliases share one route and one
  handler (`alias_of` in the parity manifest).
- Route kinds: `standard:` (stable ACP v1 projection), `core:` (typed
  `_echo_agent/*` handle-lifecycle methods), `family:` (a `<family>/op`
  surface with a closed operation list), `bridge:` (reverse extension
  invocation), `source:` (exact source identity on the generic invoke
  surface), `value:` / `intrinsic:` (serialized values / process-local
  mechanism).
- Unknown operations, wildcards, wrong signature digests and unlisted
  family operations all fail closed with typed `invalid_value` errors. The
  generic invoke surface accepts only exact catalog identities. Every canonical
  `source:` identity reaches a concrete source-operation adapter; the contract
  test rejects any return to the former generic "no Host authority adapter"
  fallback.
- A Rust source identity that reuses a family handler keeps its own source
  signature and carries an explicit `handler_operation`. Mapping is scoped to
  the exact authority type; same-named builder, server and transport methods
  cannot inherit a runtime family operation.
- Closed family routes are generated directly from `FACADE_FAMILIES`, even
  when every corresponding Rust construction type is language-local. A
  classification change therefore cannot silently remove an executable Host
  operation or its signature from the catalog.

The source-operation adapter binds immutable Agent definition
accessors (`name`, `model_name`, `system_prompt`, `tool_names`, `skill_names`,
`mcp_server_names`, `working_dir` and the definition-level `current_run_id`)
to the issued Agent handle. `AgentConfig` construction operations are
classified as language-local and use the versioned wire config DTO rather than
an invented remote Config handle. Live accessors such as `messages`,
`tool_definitions`, `disabled_tool_names`, `token_usage_summary`, context
statistics and snapshots additionally take the Host-issued Session handle and
read the same ACP Session Agent used by Prompt/Run. Runtime configuration
controls (`plan_mode`, `permission_mode`, `max_iterations`, `conversation_id`,
model/temperature/token limits, thinking and disabled tools) use that same
Session authority for reads and writes. Remaining builders and process-local
trait seams stay explicit language-local helpers; they are not fabricated as
remote handles.

The canonical `echo_core::agent::Agent::chat` and `::execute` source operations
are now thin adapters over `_echo_agent/run/start`: the request uses the
issued Agent handle plus `arguments = [Handle(Session), String(input)]`, and
the result is the normal `RunStartResponse` with Host-issued Run/Stream
handles. Event delivery, cancellation, replay and exactly-one terminal remain
owned by the existing Run authority.

`Agent::steer_input` and `Agent::steer_input_tracked` use the same pattern with
`arguments = [Handle(Run), String(text)]` and delegate to `_echo_agent/run/steer`;
the existing acceptance, conflict and settlement semantics remain authoritative.

`Agent::delegate_to`, `ReactAgent::delegate_task`,
`delegate_task_with_depth` and `delegate_to_agent_with_depth` route through the
Session Agent's captured `SubagentExecutor`/registry. The adapter preserves the
framework's first-available-subagent selection, direct-chat fallback and depth
policy. Variants carrying cancellation, runtime context, multimodal messages,
opaque prompt payloads or an explicit `SubagentAttemptIdentity` remain distinct
operations and preserve those fields through the same executor authority.

Session-bound `ReactAgent::connect_mcp_from_json` returns a Host-issued
`FacadeResource` for the framework-owned `McpClient`; `mcp_client` resolves the
same owner/generation-fenced resource and `disconnect_mcp` closes its client
resources through the shared Session teardown path. The Host never exposes an
`Arc<McpClient>` identity directly on the wire.
`load_mcp_config`, `load_mcp_from_file` and `reconcile_mcp_entry` use the same
registration helper for every client they create or replace, so replacement
does not leave an older resource silently live.

The `_echo_agent/mcp/op` surface now accepts the resulting `mcp.client`
resource for tool calls, prompt/resource reads, capability/name queries,
cached listings, ping and close. Client resources created by the source route
and family route share one integration resource map and are closed on Session
or connection teardown.

The richer delegation source routes now use the following ordered wire
arguments after the Agent handle: `SessionHandle`, `target`, `task`, optional
`parent_label`, `depth`, optional `runtime_context`, optional `allowed_tools`,
optional `prompt_payload`, and optional `prompt_context`. Multimodal variants
insert an `LlmMessageWire` after `task`. `runtime_context` is a bounded record
of conversation/run/turn/execution/isolation/message ids; `prompt_context` is a
bounded record of task title, user goal, workspace, files, execution checks,
acceptance criteria, required artifacts and constraints. Cancellation-aware
variants require an active ACP Session run and inherit its cancellation token;
they do not invent an unrelated token.

`Agent::chat_stream` and `Agent::execute_stream` use the same RunStart adapter;
their returned Stream handle is the normal bounded EventEnvelope stream, so
replay, ACK/backpressure, cancellation and terminal settlement remain shared
with the core Run profile.

`ReactAgent::chat_stream_message` and `ReactAgent::execute_stream_message` now
use the `LlmMessageWire` DTO and `TurnRequest::from_message`, preserving typed
multimodal content, tool calls and reasoning blocks before entering the same
Run/EventDelivery authority.

`Agent::close` delegates to the typed Agent close handler, including Session,
Run, facade-resource and extension cleanup; it does not merely tombstone the
opaque Agent handle.

The remaining filesystem operations call `echo_core::utils::fs` directly.
Absolute `WirePath`, `WireBytes`, `WireU64` and explicit durability values are
validated before dispatch. `ExclusiveFileLease`, `ExistingRegularFileGuard`
and `ExistingDirectoryGuard` remain real Rust values stored behind
Session-owned `FacadeResource` handles; foreign Sessions fail, and Session or
connection teardown drops the value and releases its descriptor/lease.
`facade.resource.close` releases one active resource explicitly, removes its
business record and is idempotent; a closed guard is no longer resolvable and
a closed lease can be acquired by another Session.
Pure path helpers are language-local values. `atomic_compare_and_swap` is also
explicitly process-local because its arbitrary Rust `FnOnce(&[u8]) -> bool`
predicate cannot cross ACP without narrowing the public contract.

All `SkillRegistry` operations that mutate or activate the live registry use
the Session Agent's existing registry. Prepared `SKILL.md` documents are parsed
by `SkillDocument::parse_at`; descriptors preserve location/source/hooks;
activation returns descriptor, substituted instructions and resource entries;
plugin variables retain lossless paths and string configuration. The Host does
not maintain a parallel skill catalog.

## 2. Feature model

Cargo features are the only capability authority (design §13):

- The `initialize` `_meta` advertisement derives its feature list and
  capabilities from the compiled Host feature set (`echo-sdk-host/src/features.rs`),
  never from runtime config.
- Every family surface carries a frozen feature requirement
  (`required_features` + `feature_semantics`, all-of/any-of); family methods
  and invoke routes enforce the same rules in the admission ladder, and the
  family route requirements derive from the family descriptor — not from
  per-item inventory features — so single-feature builds (`sdk-facade-adapters`
  without `sqlite`, for example) serve the state family normally.
- `sdk-facade-adapters` implies `framework-subagent`: the task family's
  `execute`/`control` and the subagent family dispatch through the
  `SubagentExecutor`, and the `task_graph` capability is only advertised
  with those handlers compiled in.
- `sdk-extension-bridge` implies `sdk-facade-adapters`. ContextCompressor
  callbacks expose a Host tokenizer resource whose canonical `count_tokens`
  operation uses `_echo_agent/facade/invoke`; sharing that admission and handle
  authority avoids a second bridge-only dispatcher.
- `improve` implies `eval`, and `framework-improve` therefore implies
  `framework-eval`: ImprovementLoop consumes EvalCase/EvalReport directly and
  cannot expose a coherent standalone public surface without them.
- `telemetry` has a concrete process-scoped adapter for `init`, `status` and
  `shutdown`.
- `channels` is advertised only when both `framework-channels` and
  `sdk-extension-bridge` are compiled. Its manager/resource operations use
  Host-issued handles, and ChannelPlugin/MessageHandler implementations use
  typed reverse bridge operations. `MessageHandler::handle_stream` uses the
  Host-minted extension stream with contiguous sequence, cancellation and
  bounded delivery semantics.
- `testing` remains a root leaf feature but is not a facade family; clients
  receive the official method-not-found rather than a false negotiated
  surface.

## 3. Task, Subagent and structured output authorities

- `_echo_agent/task/create|update|list` bind the Session Agent's own
  `TaskRevisionService`; `execute` drives the graph through
  `RuntimeTaskService` with a `FacadeTaskController` over the same
  `InMemoryRevisionedTaskStore` + `SubagentExecutor` pair, and `control`
  settles claims through the same store. One live execution exists per
  TaskRun scope: a second `execute` is a typed conflict, and cancel/pause
  reach the execution through a shared cancellation token.
- `_echo_agent/subagent/dispatch|await|control` share the same
  `SubagentExecutor`/registry as the in-conversation delegation tools;
  live dispatch records are bounded by the advertised `max_open_handles`
  (a full registry rejects the dispatch with a typed error and cancels the
  attempt).
- `_echo_agent/structured_output/validate` reuses the framework's typed
  parse path; no second Agent execution path exists.

## 4. Family operations and resources

Every family operation carries a frozen per-operation signature digest
(`operation_signatures` in the generated catalog): the Host admission
compares the request's `signature_digest` against it exactly, so an
outdated or hand-built envelope fails closed with a typed error. The
generic `_echo_agent/facade/invoke` surface is executable for the same
closed family-operation identities: `memory.store.put` through
`facade/invoke` dispatches to the memory family handler, with the same
digest and feature checks as `_echo_agent/memory/op`.

Each family method is `<family>/op` with an exact closed operation list
(frozen in the catalog):

| Family | Wire surface | Resource held per session | Required feature |
|---|---|---|---|
| memory | `_echo_agent/memory/op` | Session store or explicit InMemory/File/SQLite Store resource | — (`sqlite` for SQLite) |
| workflow | `_echo_agent/workflow/op` | compiled graphs, shared states, pull streams | — |
| state | `_echo_agent/state/op` | root state store | — |
| delivery | `_echo_agent/delivery/op` | delivery ledgers | — |
| trace | `_echo_agent/trace/op` | run stores (memory/jsonl) | — |
| eval / improve | `_echo_agent/eval/op`, `_echo_agent/improve/op` | — | `eval` / `improve` |
| permission | `_echo_agent/permission/op` | Session-owned PermissionService | `human-loop` |
| mcp / a2a / lsp / topology | `_echo_agent/{mcp,a2a,lsp,topology}/op` | managers / clients / trackers / A2A pull streams | `mcp` / `a2a` / `lsp` / `topology` |
| telemetry | `_echo_agent/telemetry/op` | process-scoped OTLP/tracing runtime | `telemetry` |
| web, files, shell, git, database, rag, chart, media, data, statistics, research, content-guard, project-rules | `_echo_agent/<family>/op` | — | root leaf feature of the family |

The canonical source operation
`echo_orchestration::runtime::turn_driver::TurnOutcome::classify` is also
executable through `_echo_agent/facade/invoke`. It has no receiver handle and
accepts exactly one `AgentEventWire` encoded as a typed `WireValue::Variant` or
`WireValue::Record` with type id
`echo_sdk_protocol::methods::AgentEventWire`. The Host converts only the
validated terminal variants to framework `AgentEvent` values and calls the
Rust `TurnOutcome::classify` authority; non-terminal variants return `null`.
Results are typed `TurnOutcome` variants (`completed`, `cancelled`, or
`failed` with an `AgentFailureWire` record), never an untyped JSON status.

Permission policy operations use the Session-owned `permission` family. The
family projects the existing `PermissionService` for mode, rule, check, cache,
and human-request prediction operations; it does not expose classifier,
request-handler, audit-sink, or responder process-local objects as remote
handles.

Family resources are Host-issued, generation-fenced
`WireHandle`s resolved through one unified authority
(`HandleRegistry::register_facade_resource` /
`facade_resource` / `close_facade_resources_of` / `close_all_facade_resources`): shape, kind, generation,
issued/closed and owner are checked on every access, and the advertised
`max_facade_resources` (default 256) is a **connection-wide global bound**
across all families — not a per-family counter. Every page bound is
`max_facade_page_items` (default 512) and `max_facade_operation_args`
(default 64) caps typed arguments before dispatch.

Resources are owner-checked against the requesting session. Closing a
session closes its resource handles in the unified authority, cancels its
task executions and subagent dispatches and drops its
workflow/state/delivery/trace/integration business records. Connection
teardown runs inside the bounded shutdown chain: cancel every task
execution and subagent dispatch and **wait for their settlement**, await
`McpManager::close_all` (child processes included), close every resource
handle and release the remaining maps.

Workflow `run_stream` and A2A `send_task_streaming` are exposed as real pull
streams. Their `*.stream.open` operation returns a Host-issued `Stream` handle;
`*.stream.next` returns one typed item or an exactly-once `complete`, `failed`
or `cancelled` terminal;
`*.stream.cancel` and `*.stream.close` are idempotent controls. Producers write
to a capacity-one queue, so a slow SDK consumer applies backpressure instead of
materializing the complete stream. `HandleRegistry` remains the only authority
for generation, owner, sequence, cancellation and close; the facade runtime
stores only the Rust receiver/background task and removes it on stream,
Session or connection close. Teardown cancels and awaits producer settlement
within the configured shutdown bound before aborting an uncooperative task.
Agent streams continue to use Run/EventReplay,
and Tool/LLM/Channel streams continue to use the typed ExtensionBridge.
Registered Workflow and SandboxExecutor implementations are exercised through
`workflow.extension.run_stream` and `shell.sandbox.extension.run_stream`;
their typed chunk/terminal unions prevent inner terminal events from appearing
as ordinary chunks. Extension Workflow events are consumed by Rust and
republished through the same canonical `WorkflowEvent` shape used by graph
streams and the same
`workflow.stream.*` / `shell.sandbox.stream.*` pull lifecycle. The temporary
facade resource that anchors this projection is closed with the stream.

## 5. Consumer trait classification

Public trait surfaces resolve to exactly one of two outcomes
(plan 08 audited closed set):

1. **typed bridge kind** — the Host has a live consumption point and a
   proxy (Tool, LlmClient, Store, HumanLoopProvider, AgentCallback,
   InterventionCallback, AgentFactory, CustomAgent, Critic, ChannelPlugin,
   MessageHandler, ContextCompressor, and the typed AgentComponent bridge for
   live persistence, audit, projection, guard, search, workflow/checkpoint,
   revisioned task, sandbox, MCP transport, embedding, memory promotion,
   intent classification and Skill load policy);
2. **explicit process-local language boundary** — the trait is a Rust generic,
   borrowed-view, `FnOnce`, marker/builder, event-bus registration or
   runtime-owned construction seam with no SDK Host call site. Each defining
   trait path has its own evidence string in the route table and remains a
   language interface/helper obligation; it is never relabeled as a same-topic
   family operation.

No public consumer trait remains under a generic `process-local-consumer`
fallback, and no `extension` item may use a family/core/invoke surface. The
generated manifest checks every canonical consumer trait and stream route,
while `intrinsic_routes_are_an_explicit_frozen_snapshot` hashes the complete
current intrinsic membership: adding a public item beneath a previously
classified type or module fails until that membership is reviewed and the
snapshot is deliberately updated.

Stateful, I/O and asynchronous service methods are not eligible for that
process-local shortcut. EvalRunner/LlmGrader, ImprovementLoop/TrajectorySaver,
PluginRegistry and InMemory/File/SQLite Store constructors and backend-specific
methods use Session-owned resources and exact source operations; ordinary Store
verbs use `memory.resource.*` against the same Rust object.

Eval and improvement adapters preserve explicit dependencies. Single-run and
grader calls take an explicit Session Agent or CustomAgent extension;
`run_all(_async)` and `ImprovementLoop::run(_async)` invoke a registered
AgentFactory lazily for each actually executed case, including early-stop.
RunStore arguments accept either a Host trace-store resource or a RunStore
AgentComponent. Eval timeout/workspace and improvement iteration/threshold/
holdout fields have get-or-copy-with-property source operations, so resource
configuration is not fixed to Rust defaults.

Plain text parameters remain lossless: an empty Rust `&str` is sent as an empty
String. Identity- or enum-specific operations validate emptiness in their own
Rust authority rather than through one global string parser. Builds with
facade adapters but no reverse bridge keep every bridge-dependent source arm
present as a typed `feature_unavailable`; CI runs that exact Host feature
combination to prevent a return to the generic fallback.

Rust-only construction/context helpers such as Agent builders/config,
`CancellationToken`, runtime snapshots, tool result/parameter helpers and
budget/timeout/retry/security policy builders are explicitly classified as
language-local. Their behavior is implemented by idiomatic SDK values and
cancel primitives; they are not assigned fictitious Host handles.

## 6. Admission ladder and error boundaries

Every facade request passes the same ladder:

1. extended-mode gate (standard connections keep the official
   method-not-found);
2. capability gate (`feature_surfaces` must be advertised);
3. request validation — exact operation identity, sha256 signature digest
   shape, no wildcards, bounded typed arguments;
4. route resolution through the embedded canonical catalog;
5. feature gate with the frozen all-of/any-of semantics;
6. family/source dispatch to the real framework service, or a typed
   `feature_unavailable`/`framework_error` that distinguishes a missing
   compiled feature from a missing source authority — never a partial or
   simulated result.

Errors carry the stable `ExtensionErrorCode` set and, for facade failures,
a `FacadeFailureDetail` with the rejected operation/digest/feature identity
(design §10.6). Long-running family work rides the official connection
task; a spawn failure during shutdown is reported as `host_shutting_down`
and never as a fake success.

## 7. Verification

- Host-side behavioral acceptance: `echo-sdk-host/tests/core_profile_e2e.rs`
  (per-family happy paths and fail-closed matrix) and
  `echo-sdk-host/tests/facade_feature_adapters_e2e.rs` (advertised resource
  bounds, duplicate `task/execute` conflict, live-cancel settlement).
- Contract/inventory acceptance: `echo-sdk-protocol/tests/facade_inventory.rs`
  and `core_rpc_contract.rs`; drift gate `scripts/check-sdk-contracts.sh`.
- CI runs the facade E2E under the `sdk-host` group with
  `--features sdk-facade-all`.
