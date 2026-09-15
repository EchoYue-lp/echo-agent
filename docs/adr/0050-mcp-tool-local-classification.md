# ADR 0050: Local MCP Tool Classification Authority

## Status

Accepted.

## Context

`McpToolAdapter` previously converted the server-provided `readOnlyHint` and
`destructiveHint` annotations directly into `ToolRiskLevel`. The adapter did not
declare matching `ToolPermission` values, so an MCP server could mark a mutating
tool as read-only and change Agent permission checks, read-only surface filtering,
cache eligibility, and failure side-effect settlement.

The MCP tools specification explicitly says clients must consider tool annotations
untrusted unless they come from trusted servers. echo-agent also serves multiple
embedding applications, so the reusable integration layer cannot assume every
configured server has the same trust level or import an EKO-specific policy.

## Options Considered

1. Continue trusting server annotations. Rejected because protocol metadata would
   remain an automatic authorization and side-effect authority.
2. Ignore annotations and classify every MCP tool as read-only. Rejected because
   unknown remote effects would still bypass read-only Agent modes.
3. Use a conservative framework default and allow an explicit locally owned
   `ToolCapabilities` decision to refine it. Accepted because it reuses the existing
   Tool capability contract and keeps application policy outside the protocol adapter.
4. Gate MCP connection behind Agent permission modes. Rejected because connection is
   a user-initiated extension action; Agent automatic invocation is the governed path.

## Decision

`McpToolAdapter` owns one local `ToolCapabilities` snapshot. Both constructors use
`Mutating`, `Standard`, and `ToolPermission::Write` by default. `Tool::capabilities`,
`Tool::risk_level`, `Tool::permissions`, and protocol-failure side-effect settlement
all read from that snapshot.

Plan tool visibility and the execution-time Plan gate consume
`ToolCapabilities.access`, not tool-name allow/deny lists. Both the explicit Agent
plan flag and `PermissionMode::Plan` produce the same snapshot-level read-only
constraint, so hook approval cannot make a locally mutating MCP tool executable.
Invocation-local `tool_search` and terminal `final_answer` explicitly declare their
non-external access classification and remain available without name exceptions.

Embedding applications may call `with_local_capabilities` after validating a tool
through local configuration or another trusted policy source. MCP annotations remain
available on the protocol `McpTool` value for display, audit, and policy input, but the
adapter never converts them into permission facts. No permission check is added to
MCP connect, reconcile, disconnect, or close operations.

## Consequences

- A forged `readOnlyHint` cannot make a default MCP tool read-only or suppress a
  possible-side-effect failure receipt.
- Default MCP tools are excluded from read-only Agent execution by the existing
  capability gate; their `ToolPermission::Write` also remains visible to the
  permission service outside Plan mode.
- Trusted deployments can still expose verified read-only MCP tools without adding a
  second permission model.
- Existing callers compile unchanged; callers that relied on annotations for automatic
  downgrades now receive the conservative classification until they opt into local
  policy.
- The change affects Agent automatic tool invocation only and does not restrict a user
  from connecting an MCP server.

## References

- MCP tools specification, annotation trust warning:
  <https://modelcontextprotocol.io/specification/2025-06-18/server/tools>
- `echo-core/src/tools/mod.rs` (`ToolCapabilities` and `Tool`)
- `echo-integration/src/mcp/tool_adapter.rs`
- `.echo-semantic/findings/finding.mcp-tool-permission-classification.md`
