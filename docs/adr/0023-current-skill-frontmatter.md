# ADR 0023: Current Skill Frontmatter Is the Only Skill Format

## Status

Accepted (single-format decision stands; the accepted field set now follows
[ADR 0026](./0026-official-skill-frontmatter-only.md), which removed the
echo-agent extension fields)

## Context

echo-agent accepted two content models in one `SKILL.md`. Current Skills stored
catalog metadata in YAML frontmatter, instructions in the Markdown body, and
supporting files in the Skill directory. An older echo-agent format also stored
`version`, `author`, `tags`, `instructions`, and a typed `resources` list in
frontmatter. Supporting both required a second instruction map, prepared-plugin
fields, registry fallback logic, and public registration methods with legacy
parameters. Applications therefore received framework compatibility state
instead of one complete Skill API.

The [Agent Skills specification](https://agentskills.io/specification) defines
`name`, `description`, `license`, `compatibility`, `metadata`, and
`allowed-tools` as frontmatter fields. Its instruction source is the Markdown
body, while scripts, references, and assets are files in the Skill directory.
The official [client implementation guide](https://agentskills.io/client-implementation/adding-skills-support)
uses the same three-tier catalog, body, and resource model. Claude Code also
[implements Agent Skills as its open standard](https://code.claude.com/docs/en/skills)
and adds product fields without creating a second instruction format. Its
cross-product packaging path rejects unexpected top-level keys instead of
silently ignoring them.

## Decision

1. `RawFrontmatter` accepts the Agent Skills fields plus documented echo-agent
   runtime extensions: `shell`, `paths`, `triggers`, `hooks`, `sandbox`, and
   `depends_on`.
2. `version`, `author`, and `tags` are not top-level fields. Authors place
   string values under `metadata`.
3. The Markdown body after frontmatter is the only instruction authority.
   `instructions` in frontmatter is rejected.
4. The Skill directory is the only resource authority. `resources` in
   frontmatter and `LegacyResourceRef` are removed.
5. Unknown top-level frontmatter fields are rejected. New framework extensions
   must be typed, documented, and deliberately added to `RawFrontmatter`.
6. `SkillLoader`, immutable plugin preparation, `SkillRegistry`, and ReAct
   registration carry only a descriptor and, for prepared plugins, the exact
   frozen document. No legacy maps, fallback data, or source-format adapters
   remain.
7. `SkillDocument` privately owns its parsed descriptor, Markdown instructions,
   and source text. The registry API distinguishes `register_descriptor` for
   lazy filesystem-backed Skills from `register_prepared(document)` for one
   validated frozen document. Callers cannot pair one descriptor with another
   document's instructions.
8. `SkillDocument::parse` and `parse_at` are the only public parsing and
   validation entry points. Framework discovery and activation, EKO catalogue
   projection, plugin validation, and install/sync validation consume this API;
   applications do not own a second frontmatter parser.

## Alternatives Considered

1. Keep the old fields with deprecation warnings. Rejected because the project
   is in active development and the compatibility path duplicated authority at
   every runtime layer.
2. Deserialize unknown fields and silently ignore them. Rejected because a
   misspelled runtime extension would appear accepted while doing nothing.
3. Normalize old fields into `metadata` or the Markdown body. Rejected because
   that is still an adapter, hides invalid source documents, and preserves two
   authoring contracts.
4. Remove every echo-agent extension and accept only the open standard fields.
   Rejected because the retained fields own real, reusable framework behavior
   rather than compatibility metadata.

## Consequences

- Framework consumers get one explicit Skill contract and two purpose-specific
  registry entry points without legacy parameters.
- Existing Skills using removed fields or non-string metadata values must be
  corrected before discovery; no migration shim exists.
- Instructions, prepared plugin identity, activation, and resource enumeration
  all derive from the same document and directory authorities.
- Product catalogues cannot advertise a Skill that the runtime parser rejects;
  installation fails before any directory replacement when validation fails.
- Future top-level extensions require a visible API and documentation change;
  arbitrary descriptive data remains available through `metadata`.
