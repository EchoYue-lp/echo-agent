# ADR 0026: SKILL.md Uses Official agentskills.io Fields Only

## Status

Accepted (amends the field set of [ADR 0023](./0023-current-skill-frontmatter.md),
whose single-format decision stands)

## Context

ADR 0023 made the "current" frontmatter the only `SKILL.md` format, but that
format still carried echo-agent extension fields at the top level: `triggers`,
`hooks`, `shell`, `paths`, `sandbox`, and `depends_on`. The
[agentskills.io specification](https://agentskills.io/specification) defines
only `name`, `description`, `license`, `compatibility`, `metadata`
(string → string), and `allowed-tools` (a space-separated string). The
official `skills-ref validate` reference tool flags everything else, so skills
written for echo-agent could not circulate in the standard ecosystem, and
standard skills could not be adopted without translation.

A `metadata.<vendor>.*` namespace was considered as a compromise and rejected
by the product decision: only the standard format is allowed — no private
frontmatter extension concepts, not even namespaced ones.

## Decision

1. `RawFrontmatter` accepts official fields only (`deny_unknown_fields`).
   Legacy extension fields at the top level now fail parsing, and the loader
   skips the skill with an explicit discovery diagnostic.
2. `metadata` deserializes as a flat string → string map. Nested maps (any
   vendor namespace) fail parsing.
3. Skill files carry no Hook field or private sidecar. Hook actions remain
   available through the host application's HookRegistry and plugin Hook
   components; their wire names match documented snake_case identifiers
   (`activate_skill`, `mcp_tool`).
4. Skill routing is description-driven: the standard format has no trigger
   field, so embedders derive routing signal from `description` text (the
   spec's own guidance). `SkillDescriptor`'s `triggers`, `paths`,
   `depends_on`, `sandbox`, and `shell` remain runtime API for programmatic
   descriptors but have no file-based source.
5. `SkillDraftGenerator` emits standard fields only (trigger patterns stay in
   curator state); `SkillMerger` persists only the merged space-separated
   `allowed-tools` string.
6. `validate_skill_markdown` / `validate_skill_dir` provide the in-process
   equivalent of `skills-ref validate` as a catalog gate: official top-level
   fields, string-shaped `allowed-tools`, string-valued `metadata`, name/directory
   match, and description limits.
7. `ReactAgent::reload_skills_from_dir` canonicalizes its directory argument
   to match the loader's canonicalized descriptor locations, making same-name
   reload an atomic remove-and-re-register.

## Consequences

- echo-agent skills are portable across agentskills.io-compatible runtimes
  and can be validated by standard tooling.
- Embedders lose file-based keyword triggers; routing quality now depends on
  description writing (mitigated by the spec's when-to-use guidance and the
  LLM intent classifier).
- `paths`-based conditional activation and declared `depends_on` are no
  longer available from files; embedders needing them must register
  descriptors programmatically.
