# Framework Architecture

## Scope

echo-agent is a reusable Rust Agent framework. It owns product-neutral Agent,
execution, state, orchestration, integration, Tool, and protocol-adapter mechanisms.
An embedding application owns its product Workspace, user experience, deployment,
and Device policy. See [Framework and Application Boundary](./39-framework-application-boundary.md).

This page explains package and ownership boundaries. It does not replace the
[configuration reference](./28-config-reference.md) or feature-specific chapters.

## Package Topology

The workspace contains the root `echo_agent` package plus eight members. Cargo
manifests are authoritative for this graph.

```text
Embedding application / protocol surface
                    |
                    v
           echo_agent root facade
        /      |       |       \
 echo_core  execution  state  orchestration  integration  tools  macros
                    |
       +------------+-------------+
       |                          |
echo-agent-learning       external SDK consumers
```

The exact member set and feature graph come from the root
[`Cargo.toml`](../../Cargo.toml). The README tables are checked against Cargo
metadata rather than maintained as a second package registry.

## Layer Ownership

| Package | Role | Dependency direction |
| --- | --- | --- |
| `echo_agent` | Stable public facade and framework composition | Consumes the seven split framework crates |
| `echo-core` | Agent, LLM, Tool, permission, event, and shared domain contracts | Foundation with no workspace dependency |
| `echo-execution` | Sandbox, Skill, and execution mechanisms | Depends on `echo_core` |
| `echo-state` | Memory, compression, persistence, and audit implementations | Depends on `echo_core` |
| `echo-orchestration` | Turn driver, Task, Subagent, Workflow, scheduler, and lifecycle primitives | Depends on `echo_core`; composes `echo_state` durable delivery primitives |
| `echo-integration` | Provider, MCP, LSP, channel, and external protocol implementations | Depends on `echo_core` |
| `echo-tools` | Reusable file, shell, web, data, media, database, and research Tools | Depends on core contracts and macros |
| `echo-macros` | Compile-time Tool, callback, guard, and handler adapters | Depends on core contracts and orchestration types |
| `echo-agent-learning` | Non-published executable consumer, examples, and documentation contracts | Consumes only the root facade |

The root [`src/lib.rs`](../../src/lib.rs) is the public composition authority.
Reasonable public framework options do not become dead code merely because one
application does not use them. A Library package is a compile-time reuse
boundary, not a runtime registry or state authority.

## Public Composition

Framework users should enter through `echo_agent` and its documented modules.
Split crates keep implementation responsibilities testable, while the facade
keeps downstream paths stable. The independent SDK repository is a consumer and
adapter; it does not move protocol state into `echo_core` or make a language SDK
the Rust API authority.

Raw Agent APIs remain valid low-level contracts. Driven surfaces such as
[Headless](./33-headless-mode.md), ACP, Eval, and the external SDK Host can compose the shared
Turn driver and receipt lifecycle. A surface may translate identity, events,
and errors, but it must not invent a second terminal or Task graph authority.

## Capability Routes

| Question | Start here | Detailed owner |
| --- | --- | --- |
| How does one Agent request run? | [Lifecycles](./lifecycles.md) | [ReAct Agent](./01-react-agent.md) and runtime Turn driver |
| How are names and identities related? | [Core Concepts](./concepts.md) | Agent, Session, Invocation, Turn, and scoped stores |
| What enters the model window? | [Core Concepts](./concepts.md) | [Context System](./40-context-system.md) and [Compression](./04-compression.md) |
| Who owns Task and Subagent execution? | [Lifecycles](./lifecycles.md) | [Tasks](./09-tasks.md), [Subagent](./06-subagent.md), and [Runtime](./29-long-running-tasks.md) |
| What is fact, checkpoint, projection, or Trace? | [Core Concepts](./concepts.md) | [Persistence Concepts](./41-persistence-concepts.md) and [Tracing](./27-tracing.md) |
| Where are Tool and Permission effects enforced? | [Lifecycles](./lifecycles.md) | [Tools](./02-tools.md), [Human Loop](./05-human-loop.md), and [Security](./security.md) |
| How do MCP, Hook, Skill, Plugin, and LSP evolve? | [Lifecycles](./lifecycles.md) | [MCP](./08-mcp.md), [Hooks](./23-hooks.md), [Skills](./07-skills.md), [Plugins](./32-plugin-system.md), and [LSP](./31-lsp-integration.md) |
| How are SDK contracts derived? | [Core Concepts](./concepts.md) | [Source-first SDK](../adr/0028-source-first-multilanguage-sdk-runtime.md) and SDK scope ADRs |
| How are effects delivered and observed? | [Lifecycles](./lifecycles.md) | [Delivery Ledger](./41-delivery-ledger.md), Outbox patterns, and [Tracing](./27-tracing.md) |
| Where do Product Backend, Frontend, Desktop, or Device concerns live? | [Application Boundary](./39-framework-application-boundary.md) | The embedding application |

## Application Boundary

The framework may accept an invocation workspace reference and provide file,
process, sandbox, or Git primitives. It does not own an EKO Workspace record,
device synchronization, GUI/TUI state reduction, product authentication, or a
deployment control plane. Those policies belong to the embedding application.
Framework Trace, Metrics, and Telemetry primitives provide diagnostics; startup,
exporter, deployment, and retention policy remain explicit consumer choices.

Protocol adapters also remain boundaries rather than owners. ACP, A2A,
Channels, Headless, and the external SDK Host project framework behavior into different
surfaces. Their current guarantees are defined by their detailed chapters and
tests; this overview does not broaden them.

## Source Authority

| Fact | Authority | Consumer check |
| --- | --- | --- |
| Package and feature topology | Cargo manifests and metadata | README documentation contracts |
| Public Rust surface | `echo_agent` facade | Facade smoke and SDK inventory |
| Runtime behavior | Source, tests, and accepted ADRs | Focused and integration tests |
| Persistent meaning | The named Store, Journal, checkpoint, or ledger | Recovery and contract tests |
| Runnable example | Cargo target metadata | `echo-agent-learning` contracts |
| SDK language scope | The independent `echo-agent-sdk` repository | SDK repository contract and language gates |

The documentation authority and its tradeoffs are recorded in
[ADR 0040](../adr/0040-framework-concept-documentation-authority.md).

## Further Reading

- [Core Concepts](./concepts.md)
- [Framework Lifecycles](./lifecycles.md)
- [Getting Started](./getting-started.md)
- [Executable quickstart](../../echo-agent-learning/examples/demo00_quickstart.rs)
- [Framework and Application Boundary](./39-framework-application-boundary.md)
