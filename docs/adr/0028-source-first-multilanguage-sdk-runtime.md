# ADR 0028: Source-First Multilanguage SDK Host on ACP

- Status: Accepted
- Date: 2026-09-04
- Owners: `echo-agent` framework
- Design: [`../supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md`](../supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md)

## Context

`echo-agent` is a reusable Rust Agent framework. TypeScript, Python, and Java
developers need idiomatic access to the complete behavior exposed by the root
`echo_agent` facade, while editors and other interactive clients also need a
standard way to invoke an echo-agent coding Agent.

The framework already owns versioned `EventEnvelope` values, finite turn
driving, exactly-one-terminal receipts, revisioned Task execution, cancellation,
recovery, Tool, LlmClient, Store, HumanLoopProvider, Subagent, MCP, A2A,
workflow, memory, and feature contracts. A transport adapter must project these
authorities rather than create another execution model.

[ACP](https://agentclientprotocol.com/protocol/overview) standardizes the
bidirectional relationship between an interactive Client and a coding Agent.
Stable protocol v1 covers initialization, Session setup, Prompt Turns, updates,
cancellation, permission, filesystem, terminal, plan, mode, and related
capability negotiation. ACP also defines a compatible extension mechanism:
custom data belongs in `_meta`, custom methods begin with `_`, and support is
advertised during initialization.

ACP does not define the complete public API of an Agent framework. In
particular, it does not provide lossless contracts for arbitrary Agent builders,
Run handles and replay, TaskRun/PlanTask/Subagent control, consumer-defined
LlmClient or Store implementations, framework journals, workflow builders, or
all echo-agent features.

## Industry Basis

- The [official ACP protocol](https://agentclientprotocol.com/protocol/overview)
  uses bidirectional JSON-RPC and models coding Agents as Client-launched
  subprocesses.
- [ACP extensibility](https://agentclientprotocol.com/protocol/v1/extensibility)
  provides namespaced custom capabilities and underscore-prefixed methods
  without changing standard fields.
- The [official ACP Rust SDK](https://github.com/agentclientprotocol/rust-sdk)
  provides schema, Agent, Client, Proxy, connection, and conformance machinery;
  TypeScript, Python, and Java libraries are also available.
- [OpenAI Codex SDK](https://developers.openai.com/codex/sdk/) wraps one local
  Agent engine and consumes structured subprocess events.
- [Claude Agent SDK](https://code.claude.com/docs/en/agent-sdk/overview) exposes
  idiomatic language APIs while retaining one Agent loop authority.

## Options Considered

### 1. Private SDK protocol plus a separate ACP adapter

Maintain one complete echo-agent JSON-RPC protocol for the SDK and another ACP
endpoint for editors. Both could call a shared service, but session, prompt,
update, cancellation, framing, capability, and conformance logic would remain
duplicated at the transport boundary.

### 2. Stable ACP v1 plus namespaced echo-agent extensions

Use official ACP as the only base Client-Agent protocol. Standard ACP clients
consume the standard profile. TypeScript, Python, and Java echo-agent SDKs use
the same connection and negotiate `_echo_agent/*` methods for facade behavior
that ACP cannot express losslessly.

### 3. Standard ACP only

Expose only ACP v1 and remove the private SDK protocol. This gives broad editor
interop but cannot meet the confirmed requirement for complete semantic parity
with the root Rust facade.

### 4. Native FFI or independent language implementations

N-API, PyO3, and JNI retain one Rust core but require three native async and
callback boundaries. Reimplementing the framework in every language creates
four authorities for execution, state, cancellation, and recovery.

## Decision

1. Choose option 2. Stable ACP v1 is the only base protocol between the SDK
   Host and all Clients.
2. Add a product-neutral, optional `acp` feature to the root `echo_agent`
   facade. It uses the official stable ACP Rust SDK and exposes a generic ACP
   Agent adapter.
3. Add a source-built process named `echo-agent-sdk-host`. The Host implements
   the ACP Agent role. The name denotes a Rust framework host, not a bundled
   Node.js, Python, or Java runtime.
4. TypeScript, Python, and Java SDKs implement the ACP Client role and prefer
   composition of the official language ACP libraries. They do not fork the ACP
   schema or reimplement standard Session/Prompt messages.
5. The first scope does not implement a generic echo-agent ACP Client, Proxy,
   Conductor, draft protocol v2, or unstable ACP features. Those are separate
   future capabilities.
6. Standard ACP clients use standard initialize, Session, Prompt Turn, update,
   cancellation, permission, filesystem, terminal, plan, mode, and command
   behavior without requiring echo-agent extensions.
   The Host launch configuration supplies one product-neutral default Agent
   definition for this profile; `session/new` fails explicitly when none exists.
7. Full SDK clients negotiate an `echo-agent` capability in ACP `_meta` and use
   `_echo_agent/*` requests and notifications for the rest of the root facade.
8. Standard ACP fields and methods are never extended in place. Echo-agent data
   that is not part of ACP uses namespaced `_meta` or namespaced extension
   methods only.
9. ACP request IDs follow the official ACP schema. Stable Agent, Session, Run,
   Event, operation, extension, generation, and idempotency identities are
   separate non-empty string fields in domain payloads.
10. One internal Session/Run service serves both profiles. ACP Session and
    Prompt Turn map to the same framework objects used by SDK handles.
11. Standard `session/update`, plan entries, tool-call status, and stop reason
    are compatibility projections of framework facts. They never become the
    TaskRun, Subagent, event, or terminal authority.
12. Complete `EventEnvelope`, Run snapshots, cursor replay, event gaps, Task and
    Subagent state, and extension receipts remain available through
    `_echo_agent/*` without being flattened into ACP fields.
13. Consumer-defined Tool, LlmClient, Store, HumanLoopProvider, Hook/Callback,
    AgentFactory, and other facade traits use the namespaced bidirectional
    extension bridge with typed identity, generation, timeout, cancellation,
    bounded concurrency, and deterministic disconnect behavior.
14. ACP permission, filesystem, terminal, and elicitation calls are made only
    when the Client negotiated the corresponding capability. Missing capability
    does not fall back to hidden local execution.
15. Keep MCP and A2A as their existing external protocols. ACP connects an
    interactive Client to a coding Agent; MCP connects Agent to tools/resources;
    A2A connects Agent to Agent.
16. Deliver ACP adapter, Host, extension protocol, TypeScript SDK, Python SDK,
    and Java SDK as source in the `echo-agent` repository. Do not publish or
    download project-built binaries, npm packages, wheels, or JARs.
17. Define SDK parity as complete functional and semantic equivalence with
    idiomatic language APIs, not Rust ABI, ownership, generic, lifetime, or
    macro parity.
18. The parity authority is every documented public item reachable from the
    root `echo_agent` facade across all public features, including the new
    `acp` feature. Internal workspace crate items remain outside that promise.
19. Maintain a machine-checked parity manifest that classifies each public item
    as ACP standard, ACP standard projection, echo-agent extension, or language
    intrinsic, and maps it to all three SDKs.
20. Reuse `EventEnvelope`, `AgentTurnDriver`, `RuntimeTaskService`, and existing
    framework services. Neither ACP nor SDK adapters own duplicate Agent, Run,
    Task, Subagent, retry, cancellation, or recovery semantics.

## Framework And Application Boundary

The generic ACP schema dependency, Agent adapter, standard-to-framework
projection, echo-agent extension contract, and conformance tests belong in
`echo-agent`. They are complete without EKO and are useful to any framework
consumer.

EKO process discovery, external Agent selection, workspace mapping, GUI/TUI
rendering, product persistence, and product permission policy belong in
`echo-agent-cli`. EKO may consume the framework ACP adapter but must not parse
ACP independently or create a second Session/Run authority.

## Implemented Adapter Contract

The first framework increment implements a transport-neutral stable v1 Agent
adapter behind the root `acp` feature. It composes the official
`agent-client-protocol` builder and typed messages rather than implementing a
JSON-RPC parser. Each `session/new` invokes an `AcpSessionFactory` and stores a
distinct framework Agent; the registry stores only protocol addressing and the
active turn cancellation token. Conversation history remains inside the Agent.

The root feature depends only on the published official ACP runtime. The
source-only, non-published `echo-sdk-protocol` workspace crate remains the
extension contract authority for the later Host and language SDKs; making it a
root optional dependency would prevent the framework feature from remaining an
independently consumable Cargo package before extension handlers exist.

`session/prompt` leaves the official serial dispatch loop before awaiting the
Agent. The spawned connection task uses `AgentTurnDriver`, an `EventSink`
projection and `TurnReceipt`; this keeps `session/cancel` and
`$/cancel_request` dispatchable while a turn is running. Both routes converge
on the same framework token, and turn identity guards prevent late cleanup from
clearing a later turn.

Text-only prompts keep the broadly compatible text Agent path. ACP ResourceLink
prompts enter the structured Message path as provider-neutral `LinkedResource`
parts, so framework Agents can inspect every field and plain user text cannot
impersonate resource metadata. Providers without a native linked-resource block
render a deterministic text fallback only at their own wire boundary.

This increment advertises only the stable initialize/new/prompt/update/cancel
baseline. `_meta.echo_agent`, optional Session methods, the configurable stdio
Host and language SDKs remain unavailable until their delivery outcomes make
the corresponding handlers real.

## Consequences

- Any standard ACP Client can invoke the supported echo-agent coding Agent
  profile without an echo-agent-specific SDK.
- The three SDKs reuse ACP transport and standard interaction types while
  retaining full facade parity through explicit extensions.
- ACP conformance and echo-agent SDK parity are independent quality gates. One
  cannot be inferred from the other.
- Standard ACP projections may be less expressive than framework state. Their
  limitations stay visible and do not weaken the full SDK contract.
- ACP absolute UTF-8 paths cannot represent every platform path. Standard ACP
  methods fail explicitly when needed; echo-agent extensions retain a lossless
  path representation.
- The project must track stable ACP compatibility separately from official ACP
  crate/schema artifact versions and echo-agent extension versions.
- Developers install Rust and their language toolchain and compile all outputs
  from one Git revision.
- Node.js/browser environments that cannot spawn a local process remain out of
  scope.
- Every root facade change carries ACP relationship, TypeScript, Python, Java,
  docs, examples, Schema, parity-manifest, and verification impact.

## Rejected Fallbacks

- ACP failure must not fall back to a private base protocol, language
  reimplementation, A2A shortcut, or EKO path.
- A standard ACP Client that did not negotiate `_echo_agent/*` must never
  receive or be required to understand SDK extension messages.
- Missing Host features return typed capability errors; SDKs do not simulate
  them.
- Extension failure remains visible to the originating framework operation;
  the Host does not substitute an unrelated built-in implementation.

## Verification Contract

- Run official stable ACP v1 conformance against the framework Agent adapter and
  source-built Host.
- Verify a non-echo-agent standard ACP Client can initialize, create/load a
  Session when supported, prompt, observe updates, cancel, and receive a stop
  reason.
- Machine-check the root facade public inventory against the parity manifest,
  including the ACP relationship classification.
- Decode the same echo-agent extension values and errors in Rust, TypeScript,
  Python, and Java.
- Run all three SDKs against a real locally built ACP Agent Host and verify the
  complete extension profile.
- Cover permission/filesystem/terminal capability absence, standard-only
  Clients, extension-version mismatch, failure, cancellation, timeout,
  disconnect, event gap, slow consumers, Host termination, restart, and late
  callback responses.
- Compile and execute ACP and language quickstarts from a clean source checkout
  without project-published or downloaded prebuilt artifacts.
