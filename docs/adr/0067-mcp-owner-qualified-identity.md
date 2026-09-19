# ADR 0067: Owner-qualified MCP server identity

## Status

Accepted

ADR 0069 reuses this typed identity while coordinating plugin transitions. The coordinator does
not derive, parse, or store a second MCP owner key.

## Context

The MCP manager historically keyed clients, configuration, prepared cleanup,
closing debt, tools, and resources by the local `mcp.json` server name. Plugin
packages are independently owned, so two enabled plugins may legitimately
declare the same local name. A withdrawal or retry for one plugin could then
close or revoke the other plugin's live connection.

## Decision

`echo-integration` owns a structured `McpServerId` consisting of an
`McpServerOwner` (`Direct` or stable `Plugin(id)`) and the canonical Unicode
local name. Every manager lifecycle map and cleanup receipt uses the owner
qualified selector as its internal key. Existing string APIs map to `Direct`
and remain source compatible. `McpServerConfig.name` and portable `mcp.json`
remain local names; authority is injected at the integration boundary.
The existing public `build_mcp_resource_tools(HashMap<String, ...>)` remains a
Direct-owner compatibility wrapper; owner-aware callers use the separate
`build_mcp_resource_tools_by_id` entry.

Plugin wiring passes `PreparedPlugin.id` into the typed Agent MCP APIs. A plugin
tool projects as `mcp__plugin_<plugin>_<server>__<tool>`; lossy Unicode or
punctuation normalization receives a stable digest suffix and collisions are
rejected before publication. Direct tools retain `mcp__<server>__<tool>`.
Resource selectors are `plugin:<base64url-plugin>:<base64url-server>`. Direct
selectors retain the old name except names beginning with reserved `plugin:` or
`direct:` prefixes, which use reversible `direct:<base64url-name>` escaping.
The resource directory stores selector-to-typed-id/client entries and never
reconstructs authority from a projected string or URI.

Plugin `mcp_tool` Hook actions name a server locally in the package. The
Integrator qualifies those names with the prepared plugin owner before Hook
registration, so the shared Hook executor always receives the typed selector.

`#72` generation and publication receipts remain the generation authority;
this ADR only supplies connection identity. `#73` coordinator and `#74`
lifecycle semantics are not moved into `McpManager`.

## Alternatives

- Reject duplicate plugin names: rejected because local names are portable
  package data and independent plugins should coexist.
- Prefix `McpServerConfig.name`: rejected because it mutates the portable
  Agent Plugins wire contract and loses the canonical local name.
- Infer ownership from tool/resource names: rejected because projection is
  lossy and resource URIs are not authority-bearing.

## Industry references

- [Agent Plugins Specification 1.0](https://agent-plugins.org/): package
  components use a stable plugin name while MCP server keys remain local.
- [Claude Code plugin namespaces](https://code.claude.com/docs/en/plugins):
  plugin skills are always namespaced to avoid collisions between plugins.
- [Codex `PluginId`](https://github.com/openai/codex/blob/78245b47af2a7aafcabe025828ceecca69db4df1/codex-rs/plugin/src/plugin_id.rs):
  plugin identity combines stable name and marketplace identity.
- [VS Code extension manifest](https://code.visualstudio.com/api/references/extension-manifest):
  extension identity is publisher plus name rather than display text.
- [Kubernetes owner references](https://kubernetes.io/docs/concepts/overview/working-with-objects/owners-dependents/):
  owner identity uses stable UID references so same-name objects do not
  interfere with one another.

## Consequences

Direct consumers keep their existing names. Plugin receipts now retain typed
identity and can withdraw one plugin without touching another. Callers that
need a stable external selector must use the returned selector; selectors are
opaque and reversible, while canonical identity remains Unicode-preserving.
