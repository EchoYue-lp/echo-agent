# ADR 0083: Share canonical tool input validation with direct adapters

## Status

Accepted.

## Context

`ToolManager` already validates a tool's JSON Schema and then invokes its custom
input validator before execution. Direct consumers that call `Tool` themselves
cannot reach that private helper. The SDK facade omitted this step: an unknown
chart type was accepted and silently generated a line chart.

Validation is a reusable framework mechanism. Positional wire argument binding
and error conversion belong to the SDK adapter. Neither requires a new registry,
executor, permission policy, or schema maintained separately from the tool.

## Reference implementations

- [MCP Python SDK Tool.run](https://github.com/modelcontextprotocol/python-sdk/blob/v1.x/src/mcp/server/fastmcp/tools/base.py)
  calls its existing argument-validation function before invoking the tool.
- [OpenAI Agents SDK function schema](https://github.com/openai/openai-agents-python/blob/main/src/agents/function_schema.py)
  associates the function with a typed parameter model and converts validated
  data into call arguments.

Both keep validation next to the authoritative tool definition. We reuse the
framework's existing implementation rather than copying it into adapters.

## Options

1. Copy JSON Schema validation into each adapter: creates multiple validation
   implementations and omits custom tool validators.
2. Construct a new ToolManager for every direct call: adds registry, timeout,
   retry and cache behavior to a boundary that only needs validation.
3. Expose the existing `validate_tool_input` helper: preserves the same schema,
   custom validator and errors without adding execution behavior.

## Decision

Make `echo_execution::tools::validate_tool_input` public. Existing ToolManager
calls stay unchanged; direct adapters call it before invoking their tool.
The existing framework re-export makes it available through the tools execution
module. No second validator, cancellation owner, or terminal authority is added.

## Consequences

Invalid input fails before tool side effects. Framework consumers remain free
to select their own execution lifetime. This Rust API does not automatically
become an accepted cross-language SDK operation; its inventory remains telemetry.
The SDK separately rejects excess positional arguments, including trailing nulls.

The helper's implementation is unchanged. Existing schema/custom-validation
regressions and direct-adapter counterexamples must pass before delivery.

## Chart dimension representation

The original GenerateChartTool stores dimensions as u32. Its schema now states
minimum0/maximum4294967295, and direct Tool execution uses checked conversion.
Defaults apply only when an argument is absent. This removes wrapping/defaulting
of supplied invalid values while retaining zero and the complete original u32
range; it adds no resource limit, public operation or parallel adapter validator.
