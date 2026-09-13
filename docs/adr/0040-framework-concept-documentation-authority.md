# ADR 0040: Framework Concept Documentation Authority

## Status

Accepted on 2026-09-14.

## Context

echo-agent has more than forty bilingual feature chapters, executable learning
examples, public facade tests, architecture decisions, and a complete semantic
baseline. The material explains individual features, but it does not provide a
stable route through the relationships among Agent, Session, Invocation, Turn,
Task, Subagent, Context, persistence, effects, extensions, and SDK surfaces.

This gap has two consequences. Readers can mistake adjacent concepts for one
global state model, and maintainers can repeat the same claim in the root README,
a concept page, and a feature chapter without a clear authority. A documentation
rewrite must not invent a generic AgentRevision, global Run, lifecycle state, or
application-owned Workspace and Device model inside the framework.

Mature agent products converge on a similar documentation shape:

- Claude Code explains the agent loop before sessions, context, permissions,
  checkpoints, and Subagents.
- Codex separates thread/session state, active turn state, and typed item events.
- Cursor treats plans as reviewable artifacts and file checkpoints as separate
  from Git; its Subagents use isolated context.
- LangGraph distinguishes thread-scoped checkpoints from cross-thread stores.
- Devin presents Shell, IDE, Browser, and progress as observable work surfaces.

These references support the conceptual separation, but echo-agent source,
contracts, tests, and accepted decisions remain authoritative for echo-agent
behavior.

## Options Considered

### Rewrite every feature chapter around one new taxonomy

This could make every page locally consistent, but it would create a large,
hard-to-review change and duplicate API and configuration details already owned
by the numbered chapters. It would also risk hiding unrelated open findings.

### Publish one comprehensive architecture and concepts page

A single page would minimize navigation, but it would mix package topology,
terminology, state authority, and lifecycle sequences into a document that is
difficult to maintain and review for bilingual parity.

### Add three foundational pages over the existing domain chapters

Architecture, Core Concepts, and Lifecycles form a small navigation layer. The
existing numbered chapters continue to own feature-specific API, configuration,
and examples. This preserves local ownership while making cross-domain
relationships explicit.

## Decision

Adopt the three-page foundational layer in both `docs/en` and `docs/zh`:

1. `architecture.md` owns package topology, framework layers, public
   composition, consumers, adapters, and the framework/application boundary.
2. `concepts.md` owns qualified terminology, identity relationships, state
   authority, persistence scope, and explicit non-responsibilities.
3. `lifecycles.md` owns cross-domain lifecycle navigation: trigger, admission,
   authority, event/effect, cancellation/failure, terminal, recovery/cleanup,
   and projection.

The root README keeps its verified feature, workspace, and example summaries.
Its Architecture section becomes a short layer view and links to the three
foundational pages. The bilingual documentation indexes are the complete
navigation authority. Numbered chapters continue to own domain details.

## Source Authority

Documentation claims follow these sources:

| Fact | Authority |
| --- | --- |
| Package and feature topology | Cargo manifests and `cargo metadata` |
| Public Rust surface | root facade and deterministic SDK inventory |
| Runtime behavior | source, tests, accepted ADRs, and resolved semantic evidence |
| Current limitation | open semantic Finding and its unique GitHub Issue |
| Runnable example | Cargo target metadata and learning contract tests |
| SDK language scope | parity manifest and source contract |

An open Finding constrains what the public documentation may promise. It does
not become user-facing prose automatically, and documentation text cannot close
the Finding. Governance artifacts remain under `.echo-semantic`; formal
framework documentation remains under `docs` in this repository.

## Concept Boundaries

- There is no generic AgentRevision. Revisions are qualified by their owner,
  such as Task revision, Plugin generation, runtime-state incarnation, or SDK
  schema revision.
- Plan is a reviewable specification artifact. The revisioned Task graph owns
  dependency, claim, execution, and settlement.
- Subagent is the only execution-role term. A Subagent has its own context,
  capability boundary, attempt identity, control, and outcome.
- Context, transcript, runtime checkpoint, long-term memory, Journal, Trace,
  Delivery Ledger, and Git or file checkpoints retain separate owners and
  persistence semantics.
- UI, feed, progress, and trace are observations or projections. They do not
  infer successful terminal state from EOF, final text, or rendering.
- Product Backend, EKO Workspace and Device policy, and GUI/TUI/Desktop state
  belong to embedding applications, not to the reusable framework.

## Consequences

### Positive

- Readers get one stable entry into architecture, terminology, and lifecycle
  relationships without losing feature-specific documentation.
- State authority and non-responsibility become explicit, reducing accidental
  parallel stores, state machines, and near-synonym APIs.
- Bilingual structure, local links, Cargo-derived README facts, and executable
  example routes can be checked by the existing documentation contract.

### Costs

- The three foundational pages and their index links must be updated together.
- Semantic changes that affect cross-domain relationships may require updates to
  both a domain chapter and a foundational page.
- Natural-language correctness still requires source-backed review; structural
  tests cannot prove every prose claim.

### Residual Risk

Open findings remain unresolved even when the foundational pages describe their
current boundary conservatively. Detailed feature chapters can still drift in
values not covered by a deterministic contract. Those problems retain their own
Finding and Issue lifecycle.

## Compatibility and Rollback

This decision adds documentation and tests only. It does not change public Rust
APIs, runtime state, serialization, protocols, SDK identities, or application
behavior. Rollback removes the six foundational pages and their navigation and
contract assertions; the existing numbered chapters remain usable.

## Validation

- Both languages contain all three pages with equivalent heading, table, and
  diagram structure.
- Root README and both documentation indexes link to them.
- All local links resolve, and referenced learning examples remain real Cargo
  examples or test contracts.
- Cargo-derived topology, feature, and README command contracts remain green.
- Semantic strict snapshot/change evidence and independent review pass without
  changing existing Finding or Issue states.

## Industry References

- [Claude Code: How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works)
- [Claude Code: Subagents](https://code.claude.com/docs/en/sub-agents)
- [Claude Code: Permissions](https://code.claude.com/docs/en/permissions)
- [OpenAI Codex: thread, turn, and item events](https://github.com/openai/codex/blob/main/codex-rs/exec/src/exec_events.rs)
- [OpenAI Codex: session-scoped state](https://github.com/openai/codex/blob/main/codex-rs/core/src/state/session.rs)
- [OpenAI Codex: active turn state](https://github.com/openai/codex/blob/main/codex-rs/core/src/state/turn.rs)
- [Cursor Agent](https://cursor.com/docs/agent/overview)
- [Cursor Plan Mode](https://cursor.com/docs/agent/plan-mode)
- [Cursor Subagents](https://cursor.com/docs/subagents)
- [LangGraph Persistence](https://docs.langchain.com/oss/python/langgraph/persistence)
- [Devin Session Tools](https://docs.devin.ai/work-with-devin/devin-session-tools)
