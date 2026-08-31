# ADR 0014: Framework Capability Placement

- Status: accepted
- Date: 2026-08-30
- Scope: `echo-agent` public framework and its application adapters

## Context

`echo-agent` is an independent Agent development framework and toolbox. EKO is
one application built on it; EKO is not the framework's only legitimate
consumer. A previous boundary rule required a second real framework consumer
before a capability could be introduced or moved into the framework. That rule
creates a circular dependency: a framework must provide complete, reusable
capabilities before outside developers can adopt them.

The framework and EKO also contain adjacent concepts with different owners.
Generic turn, task, tool, journal, plugin-preparation, routing, and lifecycle
mechanisms can be useful to many Agent products, while EKO workspace identity,
review/worktree policy, file layout, resource policy, and UI projections are
product decisions. Moving an entire coupled application module would pollute
the framework and create a second authority instead of improving reuse.

## Decision

Framework placement is decided by four tests, not by current adoption count:

1. The semantics remain complete without EKO `AppState`, workspace, Tauri, UI
   DTOs, or EKO file layout.
2. Public names and types express a product-neutral Agent concept rather than
   hiding EKO policy behind generic configuration.
3. The dependency direction remains `echo-agent <- application`; framework code
   does not import application types or product data paths.
4. The capability has framework-owned tests, examples, and documentation and
   can be independently compiled and evolved.

When a capability is accepted, one framework authority owns its generic
execution, state transition, receipt, retry, cancellation, or journal meaning.
An application adapter may convert types, add product metadata, select policy,
and project events, but it must not create a second mailbox, store, DAG loop,
retry loop, terminal reducer, or publication authority. A real EKO production
path must switch to the framework capability before the displaced implementation
is removed.

Current candidate dispositions are:

| Candidate | Framework kernel candidate | EKO-owned boundary |
| --- | --- | --- |
| AgentPool | Agent instance reuse, capacity, leases, idle eviction, and generic shutdown settlement | EKO config, workspace generation, tool visibility, model policy, and plugin target publication |
| AgentRouter | Product-neutral address/route and durable delivery lifecycle where it can reuse framework tracked receipts | EKO workspace identity, file-backed inbox layout, retirement policy, and surface projections |
| ChatEventLog | Segmented journal, checkpoint, replay, and integrity primitives | EKO chat payload, conversation identity, retention pins, and UI/channel projection |
| PluginRuntimeService | Immutable plugin preparation and validation | EKO target publication, preferences, workspace generation, and Agent fan-out |
| ExtensionControlService | Only independently reusable extension protocol primitives | EKO Skill, Hook, MCP, LSP, Browser, and Plugin mutation policy |

These are review boundaries, not approval to migrate whole modules. Each future
extraction must name exact symbols, prove its dependency direction, switch one
real path, and delete the displaced authority in the same convergence result.

Current consumer count is not a framework admission gate. It remains valid as
evidence when deciding whether to split an EKO-only contracts/domain/runtime
crate, because that is a packaging and dependency-isolation decision rather
than a framework capability decision.

## Alternatives considered

1. **Wait for a second consumer.** Rejected because it makes framework quality
   depend on adoption that the framework is supposed to enable.
2. **Move entire app-core modules.** Rejected because coupled EKO policy,
   persistence, and projections would contaminate the reusable framework.
3. **Add generic-looking wrappers without switching a real path.** Rejected
   because it preserves two authorities and leaves the boundary unverifiable.
4. **Extract only a proven kernel behind a thin adapter.** Accepted because it
   preserves framework reuse, application policy, dependency direction, and
   one authority.

## Consequences

- Framework maintainers may build product-neutral capabilities proactively for
  future users.
- Every new public capability carries a higher documentation, example, feature,
  and API-evolution obligation.
- EKO remains free to keep product-specific policy in app-core and to split an
  internal crate only when dependency and compile measurements justify it.
- Candidate work must begin with a repository-wide symbol and call-path audit;
  module names alone are not evidence of a reusable boundary.
- Existing framework capabilities such as SQLite implementations remain valid
  public options even when EKO chooses file persistence.

## References

- `echo-agent/src/agent/steer.rs` — tracked turn mailbox and receipt lifecycle.
- `echo-agent/src/agent/subagent/control.rs` — attempt-scoped Subagent control.
- `echo-agent/echo-state/src/journal/segmented.rs` — generic segmented journal.
- `echo-agent/src/plugin/prepared.rs` — immutable plugin preparation.
- `echo-agent-cli/docs/adr/0025-app-core-global-modularization.md` — EKO-only
  crate extraction evidence and R4 physical split.
- `docs/2026-08-30-framework-capability-placement-audit.md` — current
  candidate-by-candidate evidence and disposition.
