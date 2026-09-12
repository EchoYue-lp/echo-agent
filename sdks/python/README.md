# echo-agent Python SDK (source)

The Python SDK is source-only. It uses the official `agent-client-protocol`
asyncio client and launches the Host command supplied by the caller. It does
not install Python, Rust, or a prebuilt Host.
Use Python 3.10 or newer; the SDK does not install an interpreter.

```bash
uv sync
uv run python - <<'PY'
import asyncio
from echo_agent_sdk import EchoAgentClient

async def main():
    async with await EchoAgentClient.spawn(
        "/absolute/path/to/echo-agent-sdk-host",
        "--config", "/absolute/path/to/host.json",
    ) as sdk:
        agent = await sdk.create_agent()
        session = await agent.create_session()
        print(await session.invoke("echo_core::agent::Agent::name"))

asyncio.run(main())
PY
```

The same flow is checked in as `examples/quickstart.py`; the repository
language gate compiles and runs it against the source-built Host.

The wrapper preserves ACP errors and `_echo_agent/*` typed values. `RunHandle`
events are exposed as an async iterator; cancellation and close always target
the Host-issued generation-fenced handle.
`EchoAgentClient.call` and `SessionHandle.invoke` resolve canonical operation
identities through the shared catalog, including process-scoped operations
that do not require a Session handle.
`EchoAgentClient.classify_turn_outcome` delegates an explicit `AgentEventWire`
value to the framework's canonical outcome classifier and preserves raw
Variant/Null wire values.
Settled `RunHandle` instances expose `status()`, `outcome_status()`, and
`usage()`; these methods read the canonical framework receipt routes and keep
all `WireU64` usage fields as decimal strings.
The `wire_u64`, `wire_i64`, `wire_bytes`, `wire_utf8_path`, `wire_duration`
and `wire_timestamp` helpers enforce the same lossless scalar bounds before a
request is sent.
Pure local text helpers are available without a Host: `split_utf8_chunks` and
`IncrementalUtf8Decoder` preserve UTF-8 boundaries under a byte cap, while
`clean_json` and `extract_json_from_markdown` mirror the framework's JSON
cleanup and Markdown extraction behavior.
`ToolCallParams` exposes the same typed getters and required-parameter
validation. `ToolResult` is an immutable native value with
`success_result`/`success_json`/`failure_result` factories and `with_*`
modifiers; structured payloads use the shared lossless WireValue conversion.
Failure categories are validated as a closed set with the Rust recovery
mapping, and ordinary JSON objects are encoded as maps even when they contain
`kind`/`value` keys.
`TaskState` is a native string enum with the same terminal, transition and
display semantics as the Rust A2A value.
`A2AMessage`, `A2ATaskStatus`, `AgentProvider` and `AgentSkill` are immutable
native values with the corresponding text/status/provider/skill constructors.
`AgentCard.builder(...)` provides the same local immutable card and fluent
builder semantics; Agent-backed card discovery remains a Rust authority.
`A2AArtifact.new(...)` and `A2AError.new(...)` provide immutable wire DTOs for
artifact chunks and typed task errors.
`TaskStatusUpdateEvent`, `TaskArtifactUpdateEvent` and `A2AStreamResponse`
preserve the local typed stream event union without owning transport.
`A2ATaskParams`, `A2ATaskRequest`, `A2ATask`, and `A2ATaskResponse` provide
the corresponding immutable nested task envelopes.
`ThinkingLevel.parse(...)` preserves the Rust reasoning-effort aliases.
`AgentSteerState` and `AgentSteerTurnOutcome` preserve steering boundaries;
the live receipt and turn remain Host-owned.
`SubagentCommandPhase` and `SubagentStatus` preserve stable strings and parse
errors; dispatch state remains Host-owned.
`ContentGuardResult` preserves guard decisions and PII payloads; execution
remains Host-owned.
Rust `GuardResult` is projected as `GuardDecision` so the existing component
`GuardResult` helper remains source-compatible.
`DeliveryOutcome` and `DeliveryPhase` preserve stable ledger spellings without
owning delivery lifecycle state.
`SubagentStopStatus` preserves stable hook terminal values without owning hook
dispatch.
`TaskTerminalStatus` preserves stable task terminal values without owning task
execution.
`RuleSource` preserves permission source priority values and accepted aliases;
rule evaluation remains Host-owned.
`RuleBehavior` preserves allow/deny/ask payloads and `to_decision` conversion;
rule evaluation remains Host-owned.
`PermissionMode` preserves aliases and write/interaction/classifier helpers;
mode evaluation remains Host-owned.
`RuleMatcher` preserves pure tool/pattern/permission/all matching semantics;
evaluation remains Host-owned.
`CommandCellPhase` preserves stable phases and terminal classification without
owning command execution.
Command-cell terminal causes and artifact statuses preserve stable values
without owning process or artifact execution.
`TeamStrategy` preserves manager/pipeline/debate/swarm values without owning
Team dispatch.
`ConnectionMode`, `ExtensionSettlement`, and `AcpLedgerLimits` preserve ACP
runtime values without owning the connection or event ledger.
`AcpAdapterConfig` and `AcpDuration` preserve adapter limits, validation
errors, and the shutdown duration without constructing an ACP adapter.
`ExtensionLeaseError` preserves typed lease failure text while admission and
concurrency remain Host-owned.
`JwtConfig` and `JwtClaims` preserve local A2A auth configuration and subject
projection; token verification remains Host-owned.
`DependencyKind` and `SkillSource` preserve dependency/source values without
probing or loading skills.
`ContextInheritance` preserves the local inheritance defaults without owning
Subagent context, stores, or dispatch.
`ObservedIsolation` preserves trim, empty-default, and Unicode-safe bounds
without owning isolation execution.
`SegmentRange` preserves half-open saturating length and emptiness without
owning cache state.
`PromptDiagnostics` preserves section recording and per-id counts without
owning prompt compilation.
`SubagentCommandIdentity` and `SubagentAttemptIdentity` preserve durable ID
validation and projection without owning live control.
`LlmUsageStats` preserves cumulative token counters and payload projection
without owning provider execution.
`ToolOutputArtifactConfig` preserves retention, threshold, and max-age builders
without owning artifact writing.
`SkillValidationReport` preserves violation gating without running validation.
`SkillContent` preserves structured prompt-block rendering without loading or
executing resources.
JSON-RPC request and notification values preserve MCP `2.0` constructors
without owning transport.
`HookAction` preserves tagged configuration and validation without executing
hooks.
`HookEvent` and `HookEventCategory` preserve stable names, ordering, category
classification, parsing and matcher predicates without owning hook dispatch.
`EventId`, `StreamId` and `EventIdentity` preserve non-empty validation,
run/chat constructors, correlation fields and immutable `with_*` updates.
`InterventionResult` exposes immutable allow/block/cancel/inject/argument
modification factories without owning callback execution.
`TokenBudget`, `TokenBudgetConfig`, `TokenAllocation` and `LlmTimeouts` expose
the same allocation, compression and zero-disabled timeout policy helpers.
`execution_usage_duration_millis` maps absent duration to zero without owning
run accounting.
`TurnMode` preserves the chat/execute stream flavor without owning the turn
driver.
`RetryPolicy` preserves default/no-retry factories, exponential backoff caps
and optional jitter configuration without running retries.
`ThinkingConfig` preserves disabled/level/budget variants, parsing and
provider effort/budget projections without owning LLM transport.
The time helpers preserve Unix timestamps, local-offset formatting, instant
round-trips and optional null values without persisted clock state.
`ThinkingProtocol` preserves provider dialect names and field-emission
semantics without owning provider transport.
`ResourceLimits` preserves default, strict and unrestricted sandbox policy
snapshots without creating sandbox processes.
`PageInfo` preserves truncation, continuation metadata, and output projection
without owning collection state.
`SubagentContext` preserves empty/content semantics without owning context or
dispatch state.
`Usage` preserves provider-normalized cache priority and effective token
calculations without owning LLM execution.
`HookAction` preserves tagged configuration and validation without executing
hooks.
`LlmUsageStats` preserves cumulative token counters and payload projection
without owning provider execution.
`SubagentCommandIdentity` and `SubagentAttemptIdentity` preserve durable ID
validation and projection without owning live control.
Session updates and Run events are bounded async iterators with cursor ACKs,
gap/overflow errors, Host-exit propagation and idempotent close semantics.
Context compressor calls include a Host-owned tokenizer handle;
`EchoAgentClient.count_tokens` uses it for exact calibrated token counts.
Agent component calls expose exported dataclass request variants for
conversation/run/runtime state, audit, context projection, memory trigger,
guard, search, workflow/checkpoint, revisioned task, sandbox, MCP transport,
embedding and memory promotion. Named result factories preserve the same
closed nested discriminator and reject mismatched result shapes. The exported
dataclasses also include conversation ensure/search, Run append/parent-list,
IntentClassifier, SkillLoadPolicy, cancel-aware sandbox execution and
Workflow/Sandbox stream variants; stream writers validate separate typed
chunk and terminal shapes through `agent_component_chunk` and
`agent_component_complete`.

## Typed ExtensionBridge

`ToolDescriptor`, `LlmClientDescriptor`, `StoreDescriptor`, and
`CriticDescriptor` provide typed registration snapshots while the generic
`register_extension` APIs remain available. Typed callbacks receive
`ToolCall`, `LlmChatCall`, `StoreCall`, `CriticCall`, or `CompressionCall` and return
`ExtensionResultOutcome`, `CritiqueOutcome`, `CompressionOutcome`, `ExtensionStreamOutcome`, or
`ExtensionErrorOutcome` (or an existing wire mapping):

```python
from echo_agent_sdk import (
    EchoAgentClient,
    ExtensionResultOutcome,
    ToolCall,
    ToolDescriptor,
)


class SearchTool:
    async def execute(self, call: ToolCall, cancellation):
        return ExtensionResultOutcome(call.operation, {
            "kind": "text",
            "success": True,
            "output": f"search: {call.parameters}",
            "truncated": False,
        })


async def register(sdk: EchoAgentClient):
    return await sdk.register_tool(
        "search-tool",
        ToolDescriptor(name="search", description="Search documents"),
        SearchTool(),
    )
```

These classes only decode/encode the `_echo_agent/extension/*` contract; the
Rust Host remains the execution, lifecycle, cancellation, and settlement
authority. Critic callbacks receive the Host-issued `critic_critique` input
(`task`, `answer`, and `context`) and return the structured result
(`score`, `passed`, `feedback`, and `suggestions`). Verifier policy, retries,
and run settlement remain Host-owned; registering a Critic does not implicitly
enable verification.
`register_context_compressor` exposes the typed message set, integer token
limit and optional focus fields without moving compression state into Python;
the Rust Host still owns timeout, cancellation and Session teardown.
`register_agent_component` exposes the closed component/operation contract for
Host-consumed conversation, run/runtime-state, audit, context-projector and
memory-trigger implementations without moving Agent lifecycle state into Python.
