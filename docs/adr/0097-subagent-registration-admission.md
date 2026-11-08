# ADR 0097: Admit and Consume One Subagent Registration

## Status

Implementation candidate with focused Subagent qualification. The selected
runtime controls, public callback example and relevant lint checks passed;
full workspace/features, consumer qualification and exact main CI remain
separate. No SDK accepted surface or consumer pin changes here.

## Context

`SubagentRegistry` already owns atomic records containing a definition, a lazy
factory or pre-built Agent, an original `OnceCell`, and a revision. Its lazy
cache publication already fences replaced revisions. Dispatch nevertheless
looked up definition and executable separately by name. A valid concurrent
registration/removal could change the factory, timeout, inheritance or Team
member after a caller inspected the definition.

Lifecycle `SubagentHooks::before_dispatch` returns `()`. It observes a dispatch;
it cannot veto one. Outer tool checks and prompt compilation cannot establish
that the subsequently selected factory belongs to the checked definition.
Hosts need a product-neutral check on the actual selected object before any
user factory, model or background child starts. This local correctness check
prevents framework concurrency from substituting an unvalidated object; it
does not add a permission mode, GUI gate or HITL flow.

## Reference implementations

The confirmed design investigation used the OpenAI Agents SDK's
[tool execution](https://github.com/openai/openai-agents-python/blob/0dc4cf152bff1f50bdc0e41f28808563406545b2/src/agents/run_internal/tool_execution.py#L2021):
the selected `FunctionTool` is used both by input checks and by invocation.
Its [lifecycle hooks](https://github.com/openai/openai-agents-python/blob/0dc4cf152bff1f50bdc0e41f28808563406545b2/src/agents/lifecycle.py#L83)
are a separate observation mechanism. The
[Claude Agent SDK hook types](https://github.com/anthropics/claude-agent-sdk-python/blob/0e89b298e771259378a6cd3734e00d323dff5703/src/claude_agent_sdk/types.py#L511)
also distinguish pre-tool execution control from Subagent-start context
contributions. This change follows the shared selected-object pattern;
these references do not qualify Rust execution or application resource leases.

## Options

1. Recheck a name in an outer tool or observation hook. Rejected: a later
   factory lookup can select another record, and observation is not refusal.
2. Disable all dynamic registration while a host uses Subagents. Rejected:
   this removes legitimate framework/General self-extension and does not
   express the actual host execution contract.
3. Select and admit the original registry record, then consume its paired
   executable through the original dispatcher. Selected: no new registry,
   executor, store, retry loop, dependency or resource owner is required.

## Decision

`resolve_registration` snapshots one record into `SubagentRegistration` without
calling a factory. The handle retains the original factory/cache cell and an
opaque weak reference to the owning registry state. Public accessors expose
the actual definition, factory/instance availability and diagnostic revision.
`is_same_registration` compares registry and original record identity. The
revision is process-local and must never become a persisted plugin version.

An optional `SubagentRegistrationAdmission` is configured on the original
registry. All its executors, including existing Agent-created dispatchers,
share that configuration. `admit` receives the selected record and the
original `DispatchRequest`, including `runtime_context`, run identity and
existing invocation guards. The framework checks registry/target ownership
first, clones the callback under its configuration lock, then releases all
registry/configuration locks before awaiting host code or invoking a factory.
Hosts own their provider/content/run interpretation. No EKO field enters the
generic API. A policy must not retain strong references back to its long-lived
Agent/registry or keep invocation resources beyond its own scoped ownership.

`agent_tool` resolves once for isolation/mode/background routing and passes
that same handle to the original registered dispatch entry. Sync, Fork,
Teammate and Team root/member selection consume the admitted record. Every
retry or transfer to a new target resolves and admits its actual record anew.
Fresh factory instances still take precedence over a cached instance, and
dispatch never publishes a fresh result into another registration's cell.
The original public lazy-cache lookup and its singleflight/publication fences
remain unchanged; they are raw construction APIs, not policy-bearing dispatch.

Background admission happens before the spawn/handle return. Its admitted
first record is consumed once, without a second policy call in the spawned
owner; later retries are checked anew. A refused controlled-background attempt
settles its original reservation without creating an Agent. Existing exact
attempt, cancellation, tool scope, depth, isolation, physical-settlement and
invocation-guard authorities remain in place.

The explicit public `dispatch_teammate().join()` entry also reuses the original
non-process keyed resource bundle. Only that direct entry transfers this
bundle to the already-existing Teammate physical task; `dispatch_inner` keeps
its own original bundle and does not allocate another one. Stream-setup
`ExecutionNotSettled` can occur before the Agent default wrapper constructs a
retaining stream, so a context/handle alone cannot retain the last guard.
Unknown settlement or unwind transfers it to the same original keyed debt
owner; known original EOF releases it. Dropping a handle requests cancellation
and does not abort that original physical task or release its guards early.

Team members receive the original parent invocation's real guard clones in
their original runtime context. The retained Team dispatch closure holds only
a weak reference to that invocation-scoped bundle; actual dispatch must
upgrade it while the original call is alive, before policy/factory consumption.
An expired guarded source refuses new members. This avoids a completed,
retained runtime handle indefinitely holding the old pool lease. Calls that
never supplied guards keep their original behavior. This is propagation of
the existing guard carrier, not another lease/slot/cleanup authority.

A same-name replacement or removal affects future selections. An already
admitted invocation may finish with the original record; it never silently
switches to the replacement. Passing a handle from another registry, even
with the same name/revision, is refused. Configuring a new callback affects
subsequent admissions and does not revoke already admitted in-flight work.
Consumers needing atomic product publication continue to use their original
generation/lease owner; this handle is not a frozen historical environment.

## Consequences

- No callback means the original dynamic-registration/fresh-dispatch behavior.
- Existing by-name executor APIs now take the same checked record path.
- Host refusal precedes user factories and actual child execution. It is
  returned through the existing error channel, not a new runtime state.
- Public Rust additions require normal feature/API verification. They do not
  automatically become language SDK accepted contracts or update its pin.
- The communication example demonstrates admission with original attempt
  identity. The public Subagent guides and website mirror need to stay aligned.

## Verification and rollback

Regression definitions run actual executor, agent_tool and Team paths. Held
semaphores interleave policy checks or factory construction with registration
replacement/removal. They check exact definition/executable pairing, refusal
before zero factory calls, no stale result in the replacement's lazy cache,
foreign-registry handles, cancellation during policy wait, fresh retry/retarget
checks, controlled-background claim settlement and default General extension.
Existing physical cleanup, resource-guard, claim and cancellation tests remain
part of qualification. Definitions and formatting alone are not runtime
evidence; actual validation uses an explicitly granted Cargo batch, followed
by saving artifacts and cleaning its exact owned target.

The focused `--no-default-features --features subagent` batch passed 211
Subagent tests and 22 original `agent_tool` tests: 233 distinct tests, including
all 15 new definitions, with zero failures or ignored tests. Three controlled
restorations of the faulty execution paths each ran one selected test and
failed its behavior contract: a replaced-name factory, missing Team guard
propagation and the direct Teammate's missing keyed retainer. Compilation
errors and zero selected tests do not count as those counterexamples.

The actual `demo50_subagent_communication` run admitted the expected attempt,
delivered both uplinks and completed its terminal replay. Library/test Clippy,
production panic/unwrap/expect/unreachable checks, example Clippy, featureless
library compilation and final formatting checks passed without compiler
warnings. The owned target was cleaned after original owners/readers settled.
These selected results qualify this framework slice only. They do not inherit
the predecessor's full matrix or prove EKO/SDK integration, native services,
all features, release readiness or a future main commit's CI.

Rollback is a normal source revert without persisted migration. It restores
the separate-name-lookup race and removes host admission; it must not be
described as equivalent enforcement.
