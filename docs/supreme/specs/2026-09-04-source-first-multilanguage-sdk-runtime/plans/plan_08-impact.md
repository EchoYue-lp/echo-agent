# Plan 08 Impact Conclusions

## SDK-Docs-Impact

`update`: the Rust Host facade contract, ACP extension schema, channel bridge
operations, source-operation session accessor semantics, and generated catalog
were changed. The canonical updates are in `docs/sdk/`, the ADR, the README
status section, CHANGELOG, and the checked-in `contracts/sdk/` artifacts.
At the time of the Plan 08 Host closeout, language SDK documentation remained
explicitly `not_implemented`; the current language work closes only the
executable route baseline and does not change that historical Host outcome.

The typed extension contract now includes the live `ContextCompressor`
consumption point. TypeScript, Python and Java expose idiomatic typed callbacks
over the same Host invocation authority; no language SDK owns compression
state, cancellation, timeout or teardown.
The callback receives a temporary Host tokenizer resource and can invoke the
canonical count operation, preserving calibrated token accounting. Agent
component calls/results are operation-discriminated schema unions; Java uses a
sealed request hierarchy, and all three SDKs preserve the same nested
discriminator and canonical integer forms.

The Host facade now exposes the canonical `TurnOutcome::status`,
`TurnReceipt::status`, and `TurnReceipt::usage` source identities through a
Host-issued `Run` receiver.
Settled live runs read the existing `RunEntry` receipt and recovered runs read
their persisted `RunReceiptWire`; both projections preserve textual `WireU64`
counters and reject running or receipt-less runs with typed errors. This is a
Host-only authority projection and does not add protocol/catalog or Skill
surface changes.

`TurnOutcome::classify` is served by a typed source adapter that accepts only
the closed `AgentEventWire` value and delegates terminal classification to the
framework. `TurnReceipt::cancelled` and `TurnReceipt::failed` remain explicit
language-local pure constructors: they create process-local receipt values and
do not identify a Host-owned Run, so advertising them as remote operations
would invent a second receipt authority.

The Session-owned PermissionService now has a dedicated `_echo_agent/permission/op`
family. Mode, check, rule, cache and human-request prediction operations resolve
through the exact Session Agent PermissionService used by the execution
pipeline; the adapter never constructs a parallel policy state.

The final Plan 08 diff also closes every catalogued `source_operation` against
a concrete Host adapter. Durable filesystem functions call the existing Rust
authority; lease and file/directory identity guards use Session-owned facade
resource handles. Workflow and A2A expose capacity-one pull streams with
Host-issued Stream handles, serialized `next`, typed terminal events and
explicit `cancel/close` operations. `facade.resource.close` releases active
guards, leases and family resources through the unified handle authority. Rust
source identities that reuse a family handler carry an exact, type-scoped
`handler_operation`; builder/server/transport methods are not matched by name. The
formal SDK references, ADR, README/README.zh and CHANGELOG were updated. No
standalone example was added because the new wire behavior is covered by real
official-Client Host E2E; `echo-website` remained unchanged at the Plan 08
Host closeout because overall three-language Parity complete was still false
at that point.

## Subsequent multilingual route-baseline closeout

The source-only TypeScript, Python and Java SDKs now map every executable
catalog route to the canonical resolver and preserve serializable values with
the lossless WireValue algebra. Process-local intrinsic items remain
explicitly `not_implemented` until their language-native behavior and evidence
are delivered. The cross-language gate checks the executable route mapping,
all catalog operation identities/signatures, and the three source test suites
against a source-built `sdk-facade-all` Host; it does not yet support an
overall Runnable or Parity complete claim. TypeScript and Python quickstart
examples are compiled and run by the same gate; intrinsic operation helpers
remain the outstanding language outcome.

Consumer traits with a live SDK Host consumption point now use a typed bridge.
No public `extension` item is relabeled as a same-topic family/core route.
Rust generic, borrowed-view, `FnOnce`, marker/builder, event-bus registration
and runtime-owned construction traits without a Host call site carry an exact
process-local reason and remain language interface/helper obligations. A frozen
membership digest forces explicit review when that set changes.

Stateful/I/O public methods called out by strict review are now Rust-owned
resources and exact operations: the complete EvalRunner/LlmGrader,
ImprovementLoop/TrajectorySaver, PluginRegistry and concrete Store method sets
no longer use process-local intrinsic routes. The stateful resource E2E proves
same-owner state visibility, foreign-Session rejection, explicit close and
closed-handle behavior.

Strict-review remediation extends AgentComponent to every overridable
ConversationStore, RunStore, SandboxExecutor and Workflow method and adds the
live IntentClassifier and SkillLoadPolicy bridges. Skill discovery, prepared
registration and reconciliation await the same full descriptor callback.
Workflow operations alone use exclusive admission; ordinary Send+Sync
component calls remain concurrent. Sandbox component streams separate typed
chunks from typed terminals, reuse the canonical Workflow projection and
retain typed cancellation after cleanup. Eval and
improvement use explicit Agent/AgentFactory/RunStore inputs and lazy factory
creation, with public resource configuration fields. Empty text stays
lossless. MCP client/tool publication now rolls back as one transaction;
failed initialization and post-initialize handle publication failure close the
transport, and on-demand notifications are bounded. A no-bridge Host
combination test proves bridge-dependent source operations return typed
feature-unavailable rather than the generic fallback.

The first language-native intrinsic slice now covers the 28 canonical
`ToolCallParams`/`ToolResult` wire-helper entries in TypeScript, Python and
Java. Each implementation has focused constructor, modifier and validation
tests, and the manifest marks only this exact slice `done`; process-local
authority and the remaining intrinsic behavior stay explicitly open.

The same route-baseline increment also closes the 10 canonical A2A `TaskState`
value identities (terminal/transition behavior and Display) in all three
language SDKs; no Host or wire authority is introduced.

The follow-on native-only A2A value slice closes 14 canonical Message,
TaskStatus, Provider and Skill constructor/projection identities in all three
language SDKs; these values remain immutable projections with no new Host,
network or lifecycle authority.

The Agent Card increment closes the local card and fluent builder identities in
all three SDKs while retaining `from_agent` as an explicit Rust-authority
boundary.

The A2A Artifact/Error increment closes eight immutable wire-value identities
without introducing a Host or network route.

The A2A stream value increment closes 18 typed event/response identities while
leaving stream transport and lifecycle authority in the Host.

The task envelope increment closes 15 nested request/response identities while
leaving task execution and lifecycle authority in the Host.

## SDK-Skill-Impact

`update`: `ReactAgent::discover_skills` is now a canonical source operation
with a bounded wire representation for custom, project and user discovery
scopes. The Host delegates discovery to the existing framework SkillLoader;
the facade adapter docs and generated catalog record the operation, while no
new Skill execution authority or language-SDK status claim is introduced.

The remaining public `SkillRegistry` accessors, mutations and activation paths
now resolve the existing registry of the addressed Session Agent. Prepared
documents continue through `SkillDocument::parse_at`, plugin variables retain
lossless paths, and activation returns the framework descriptor/instructions/
resource projection. SkillLoadPolicy is now an awaited framework callback so
the same Host-language policy can gate discovery, prepared plugin registration
and reconciliation without blocking the runtime; its full descriptor input is
statically typed in TypeScript and shape-validated in Python and Java. This
changes the SDK Skill surface and its documentation, but does not move
discovery, validation, activation ordering, sandbox policy or execution
authority out of the Rust framework.
