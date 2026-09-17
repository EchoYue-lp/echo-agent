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
dependency-ordered `PreparedPluginSet` with a monotonic generation, deterministic content identity,
structured diagnostics, parsed Skills/Hooks/MCP, and owner-qualified frozen Subagent/LSP documents.
`wire_prepared` and rollback consume only this set and perform no component filesystem reads.
`PluginWiringResult` remains only an apply/unwire receipt. EKO monitors, themes, output styles,
workspace fanout, and UI receipts remain application policy.

Preparation isolates failures at the component boundary. An invalid Skill, Hook, or MCP component,
or an unreadable frozen Subagent/LSP document, is omitted from its `PreparedPlugin`; an error
diagnostic retains the plugin, component, path, and cause. Embedding applications validate their
product-specific Subagent/LSP syntax in a second preparation stage. Healthy sibling components and
plugins remain in the same snapshot. Component diagnostics do not make the generation inapplicable;
only a generation-wide failure that prevents a complete dependency-ordered snapshot (for example,
dependency resolution, whole-plugin preparation, or generation allocation failure) rejects it. The
complete `PreparedPluginSet` remains one immutable input to publication. Active-generation fencing,
replacement ordering, and owner-scoped withdrawal are separate responsibilities tracked by Findings
#72 through #75.

## Alternatives

- Per-target live scans: rejected because targets can observe different bytes.
- EKO fields in the framework snapshot: rejected as product coupling.
- Unbounded revision cache: rejected; only the latest set per registry remains cached while existing
  `Arc` snapshots remain usable by active consumers.

## Consequences

Explicit registry mutation or invalidation advances generation. Equivalent bytes retain the same
identity. Invalid dependency or generation-wide errors make a set non-applicable, while component
parse/read errors are isolated and wiring remains deterministic.

## References

- [Codex PluginsManager](https://github.com/openai/codex/blob/cdde711fac008cd4e1115603ead713cf23b1a580/codex-rs/core-plugins/src/manager.rs#L398-L506)
- [Codex shared managers](https://github.com/openai/codex/blob/cdde711fac008cd4e1115603ead713cf23b1a580/codex-rs/core/src/thread_manager.rs#L258-L276)
- [Codex SkillsManager](https://github.com/openai/codex/blob/cdde711fac008cd4e1115603ead713cf23b1a580/codex-rs/core-skills/src/manager.rs#L51-L121)
- [Claude Code plugins](https://code.claude.com/docs/en/plugins)
