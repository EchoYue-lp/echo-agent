# ADR 0011: Typed Tool Output Artifact

- Status: Accepted
- Date: 2026-08-29

## Context

`ToolResult` already carried typed success, failure, truncation, MIME, and JSON
facts. Complete output that exceeded the inline budget was also represented by
`ToolOutputArtifactRef`, but producers encoded that descriptor into fixed keys
inside the open-ended `metadata` map. Trace, Subagent, application, CLI, and TUI
consumers then independently reconstructed path, byte count, digest, and
retention. Some consumers also called `Path::is_file` and created a new local
availability fact.

This made a generic framework fact look like application metadata. A missing or
partially copied key silently changed behavior, and every surface could disagree
about the same terminal tool result.

Established implementations keep tool output and resource references typed:

- MCP `CallToolResult` returns typed `ContentBlock` values, including
  `ResourceLink` and `EmbeddedResource`, instead of requiring clients to decode
  resource identity from text metadata.
- OpenAI Agents Python represents `ToolCallOutputItem.output` separately from
  its raw provider item, so downstream processing receives an explicit tool
  output contract rather than parsing the raw record.
- Codex protocol events use typed dynamic-tool content and file-change events,
  keeping structured output facts distinct from display text.

The common pattern is not a product-specific artifact store. It is a typed
terminal result whose referenced content can be validated by the owning host.

## Considered Options

1. Keep artifact keys in `ToolResult.metadata` and centralize a decoder. This
   still makes an open-ended string map the artifact authority and permits
   partially formed descriptors.
2. Add an application-only descriptor. Framework trace, Subagent, and other
   consumers would still need a parallel interpretation.
3. Put `Option<ToolOutputArtifactRef>` on canonical `ToolResult` and reuse the
   same descriptor in output processing, trace, Subagent, and applications.

## Decision

Adopt option 3.

`ToolResult.artifact` is the only complete-output artifact fact. Producers set
it directly, `ProcessedToolOutput` carries it through the spill stage, and
`RunEvent::ToolResult` stores the same typed descriptor. `metadata` remains for
open-ended measurements and tool-specific annotations, but it must not contain
a second artifact descriptor.

`ToolOutputArtifactRef::extend_metadata` and `from_metadata` are removed. There
is no compatibility decoder because the project is pre-release and all
workspace consumers migrate in the same iteration.

The descriptor means that artifact creation completed and records its path,
stored bytes, original payload bytes, SHA-256, and retention label. Consumers
must not replace this producer fact with a local path-existence check. A host
that exposes or reads the artifact may additionally validate the registered
root, retention, file identity, size, and digest; that validation is host
policy, not another descriptor authority.

## Consequences

- Tool events, trace, Subagent observation, replay, and every surface preserve
  one lossless descriptor.
- Missing descriptors are explicit `None`; partially copied metadata cannot be
  mistaken for an artifact.
- Applications retain ownership of storage roots, retention, access policy,
  and verified readers.
- Adding the public field is intentionally source-breaking for explicit
  `ToolResult` literals; workspace consumers are updated together.

## References

- MCP schema 2025-06-18, `CallToolResult` and typed content/resource blocks:
  <https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2025-06-18/schema.ts>
- OpenAI Agents Python, `ToolCallOutputItem`:
  <https://github.com/openai/openai-agents-python/blob/main/src/agents/items.py>
- OpenAI Codex protocol typed dynamic-tool and file-change events:
  <https://github.com/openai/codex/blob/main/codex-rs/protocol/src/protocol.rs>
