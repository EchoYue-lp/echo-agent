# ADR 0071: Protected Paths and Read-Only Tool Boundaries

## Status

Accepted

## Context

The Agent permission pipeline accepted Hook Allow before consulting the
`PermissionService` protected-path checker. A read-only Agent filtered the
standard tool pack during construction, but still registered custom mutating
tools. Both paths could allow an automatic tool effect outside the declared
boundary.

## Options Considered

1. Copy protected-path patterns into the Agent pipeline and maintain a second
   read-only tool-name list. Rejected because the two decisions could drift from
   their existing authorities.
2. Invoke the existing protected-path decision before Hook Allow and reuse
   `ToolCapabilities` for read-only admission. Accepted because the same facts
   govern the service, tool surface, and default execution pipeline.

## Decision

`PermissionService` exposes its protected-path decision for the effective tool
input. The Agent checks it after `PreToolUse` has applied input rewrites and
before either Hook Allow can return. Protected paths continue to apply in all
permission modes. The service records the denial in its configured permission
audit sink exactly once, whether the check came from normal policy or the Hook
path. Handler-provided input rewrites use that same check and audit the effective
rewritten input rather than the original request.

`readonly_tools` filters custom tools during Builder construction. Invocation
tool definitions and the default execution pipeline also check capabilities,
so tools registered after construction cannot enter the read-only Agent's
automatic tool path. This is a framework mechanism independent of an embedding
application's approval policy.

Framework observation tools retain read-only capability declarations when
their implementations only load or snapshot state: `task_list`, `list_cells`,
and `subagent_list`. Store-backed `recall` and `search_memory` remain mutating
because `MemoryRecaller` asynchronously writes persistent recall telemetry.
Layered memory search can reconcile pending changes and also remains mutating;
task graph updates, cell stop, and Subagent messaging remain mutating as well.

## Consequences

- A Hook can allow an ordinary tool without bypassing a protected-path denial.
- Read-only custom tools remain usable; mutating custom tools are excluded from
  construction and later invocation.
- Capability declarations remain the tool author's responsibility. This change
  does not add a separate permission authority or affect direct user actions.

## References

- [ADR 0050](0050-mcp-tool-local-classification.md)
- `.echo-semantic/findings/finding.hook-protected-path.md`
- `.echo-semantic/findings/finding.readonly-tools-custom-registration-bypass.md`
