# ADR 0012: Immutable plugin preparation generations

## Status

Accepted

## Context

Plugin wiring previously reread package files while mutating live Agents and during rollback. One
reload could therefore expose different filesystem states to different targets.

OpenAI Codex shares cached plugin and Skill managers and keeps disk changes invisible until explicit
force reload at commit `cdde711fac008cd4e1115603ead713cf23b1a580`. Claude Code plugins similarly
load a bundle and expose explicit reload rather than making every consumer rescan it.

## Decision

`PluginIntegrator` owns preparation and its bounded cache. `prepare` returns an immutable,
dependency-ordered `PreparedPluginSet` with a process-wide checked monotonic generation,
deterministic content identity,
structured diagnostics, parsed Skills/Hooks/MCP, and owner-qualified frozen Subagent/LSP documents.
`wire_prepared` and rollback consume only this set and perform no component filesystem reads.
`PluginWiringResult` remains an apply/unwire receipt, now bound to an Agent's publication target,
generation, content identity, and privately issued token. The canonical target authority lives on
the Agent; cloned integrators share preparation but cannot publish a generation for another Agent.
Independent integrators receive ordinals from the same allocator, so their snapshots can be ordered
on the same Agent. The allocator is not active publication state; Agent targets remain independent.
EKO monitors, themes, output styles, workspace fanout, and UI receipts remain application policy.

Preparation isolates failures at the component boundary. An invalid Skill, Hook, or MCP component,
or an unreadable frozen Subagent/LSP document, is omitted from its `PreparedPlugin`; an error
diagnostic retains the plugin, component, path, and cause. Embedding applications validate their
product-specific Subagent/LSP syntax in a second preparation stage. Healthy sibling components and
plugins remain in the same snapshot. Component diagnostics do not make the generation inapplicable;
only a generation-wide failure that prevents a complete dependency-ordered snapshot (for example,
dependency resolution, whole-plugin preparation, or generation allocation failure) rejects it. The
complete `PreparedPluginSet` remains one immutable input to publication. On one Agent, a newer
generation may publish only after the current receipt withdraws; the target rejects stale prepared
sets and stale, altered, or foreign receipts before registry mutation. Applying the same generation
again is a typed refusal. Apply failure does not advance the published generation, and failed
cleanup retains the issued receipt for retry. A cancelled apply leaves a pending receipt and blocks
later publication until its owner explicitly settles it. The target does not coordinate registry
persistence with callbacks, nor does it add MCP owner isolation; those remain #73 and #75.
Each successfully connected MCP server is recorded in the canonical receipt before the next
server begins. For a previously absent name, the pending receipt reserves its cleanup scope before
the awaited connection, covering cancellation after the manager publishes but before the Agent
returns. A failed new connection settles its own scope or retains cleanup debt; it does not claim
ownership of a pre-existing same-name target.

## Alternatives

- Per-target live scans: rejected because targets can observe different bytes.
- EKO fields in the framework snapshot: rejected as product coupling.
- Unbounded revision cache: rejected; only the latest set per registry remains cached while existing
  `Arc` snapshots remain usable by active consumers.
- Integrator-global active generation: rejected because one cloned integrator prepares for several
  independent Agent targets. Generation numbers are compared within the Agent target only.
- Per-Integrator generation counters: rejected because unrelated counter value `1` cannot supersede
  another integrator's published value `1`, and an older value `3` can overtake a newer value `2`.
  The checked allocator orders prepared snapshots for one process lifetime; target authority still
  owns admission and receipt settlement.
- Automatically replace an active generation: rejected because side-effect cleanup must settle
  before the new generation can be published (see ADR 0060).

## Consequences

Explicit registry mutation or invalidation advances generation. The process-wide ordinal also
advances when an independent integrator prepares its first snapshot. Equivalent bytes retain the same
identity. Invalid dependency or generation-wide errors make a set non-applicable, while component
parse/read errors are isolated and wiring remains deterministic. A holder of an old `Arc` snapshot
may still inspect it, but cannot publish it after its target has advanced. Existing integrator
apply/rollback calls resolve that same per-Agent target rather than bypassing the authority.

## References

- [Codex PluginsManager](https://github.com/openai/codex/blob/cdde711fac008cd4e1115603ead713cf23b1a580/codex-rs/core-plugins/src/manager.rs#L398-L506)
- [Codex shared managers](https://github.com/openai/codex/blob/cdde711fac008cd4e1115603ead713cf23b1a580/codex-rs/core/src/thread_manager.rs#L258-L276)
- [Codex SkillsManager](https://github.com/openai/codex/blob/cdde711fac008cd4e1115603ead713cf23b1a580/codex-rs/core-skills/src/manager.rs#L51-L121)
- [Claude Code plugins](https://code.claude.com/docs/en/plugins)
- [Kubernetes API resource versions](https://kubernetes.io/docs/reference/using-api/api-concepts/#resource-versions)
- [ADR 0060 callback cleanup settlement](0060-plugin-lifecycle-reconcile-settlement.md)
