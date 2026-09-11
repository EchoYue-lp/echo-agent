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
