# ADR 0096: Selective prepared plugin content identities

## Status

Accepted for implementation. This source contribution is not yet Rust-qualified or delivered to main.

## Context

ADR 0012 owns a same-read immutable plugin generation, original registry dependency validation,
and per-Agent publication/cleanup. Its whole-set identity changes when any member changes.
An embedding application needs the content identities of a selected plugin and its real declared
closure without treating every installed plugin, a version label, or a process ordinal as that
selection's content identity. Optional manifest versions must retain their original meaning.

The approved task-bound domain design assigns this reusable mechanism to the framework and
product rule parsing/admission to the application. It does not authorize a second manager,
validator, version archive or resource owner. ADR 0012's Codex/Claude reload references provide
the existing immutable-preparation rationale; the application design's Temporal/LangGraph
comparison motivates detecting content drift rather than silently reinterpreting persisted work.

## Alternatives

- Use whole-set identity: rejects unaffected work when an unrelated plugin changes.
- Use name/version or generation: misses same-version edits or changes after process restart.
- Rescan files/resolve dependencies in each consumer: creates competing input and validation authorities.
- Archive complete old packages/resources: outside this mechanism and not provided by holding an Arc.

## Decision

Add `PreparedPlugin::content_identity()` and
`PreparedPluginSet::dependency_closure(plugin_id)` to the existing facade. Original prepare
builds each identity from the same manifest/configuration and frozen documents that it publishes.
Canonical JSON object keys and manifest dependency order prevent HashMap/insertion-order drift;
array order otherwise remains meaningful. The original variable substitution also orders its existing
user-config pass by key, so references inside values cannot change frozen text with HashMap order.
Built-in → user-config → environment stages remain unchanged; this adds no recursive expansion
or new variable resolver. Identity normalization never hides differing published instructions.
Documents include their component kind, relative source
and bytes, using the existing length-delimited hash framing. Each plugin keeps its validated
outgoing dependency names; selection only walks that frozen graph and filters the original stable
dependency-first sequence. Original registry checks remain the sole presence/enabled/version/cycle
validator. Invalid generations and missing roots cannot return a usable closure.

Whole-set identity continues to describe complete publication, aggregating all canonical plugin
identities. This changes its digest encoding, not its owner or admission responsibilities; previous
strings are not persistent cross-version API versions. Publication targets and issued receipt
checks remain untouched. Single-plugin identity excludes dependency contents so consumers can
retain each real provider identity; the returned closure supplies transitive dependencies explicitly.

Coverage is limited to the original framework's prepared inputs, including discovered Skill identity
documents, Hook/MCP declarations and frozen Subagent documents. Existing frozen adapter inputs
continue their original feature handling until their separately approved retirement is integrated.
The mechanism does not freeze unread helper scripts, complete references directories, external
commands/services, model results or physical side effects. It contains no EKO policy or credentials
projection. Applications retain responsibility for their own same-read components and actual
invocation admission; an old prepared Arc is observation, not authority to use retired resources.

## Consequences and validation

Extend the existing plugin example instead of adding a new demo count/target. Retain existing
publication, stale generation, cancellation, rollback and cleanup tests. Add selection controls
for transitive changes versus unrelated changes, stable reprepare/restart and dependency order,
absent versions, original dependency refusals, nested object order and unread Skill helper boundaries.
Docs in both languages describe the coverage and the aggregate encoding change.

This source phase runs no Cargo, rustfmt, metadata or native process and creates no target.
Focused Rust/example qualification and any applicable feature/full gates run only in the
coordinator's resource window; logs must be retained outside the precise target before its guarded
cargo clean. SDK pin/accepted contracts and website publication are not changed by this contribution.

## References

- [ADR 0012](0012-immutable-plugin-preparation.md)
- [ADR 0069](0069-plugin-host-lifecycle-coordinator.md)
- Cross-repository approved design: `eko-domain-plugin-runtime/design.md`, sections 3, 5, 7, 10 and 12.
