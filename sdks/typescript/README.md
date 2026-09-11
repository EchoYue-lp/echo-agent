# echo-agent TypeScript SDK (source)

This directory contains the source-only TypeScript ACP Client for `echo-agent`.
It uses the official `@agentclientprotocol/sdk` v1 client and starts the Host
executable supplied by the caller. No Host binary, Node runtime, or npm
artifact is bundled.
The source client requires Node.js 20 or newer (the compiler target is
ES2022).

```bash
npm install
npm run build
node --input-type=module -e '
import { EchoAgentClient } from "./dist/index.js";
const sdk = await EchoAgentClient.spawn({
  hostCommand: "/absolute/path/to/echo-agent-sdk-host",
});
const agent = await sdk.createAgent();
const session = await agent.createSession();
console.log(await session.prompt("Reply with one word."));
await sdk.close();
'
```

The same flow is checked in as `examples/quickstart.ts`; `npm test` compiles
it, and the repository language gate runs the emitted example against the
source-built Host.

`EchoAgentClient.invoke` and `family` resolve signature digests from the
canonical checked-in catalog. `AgentHandle`, `SessionHandle` and `RunHandle`
are opaque generation-fenced handles; streams are exposed as `AsyncIterable`.
Use `EchoAgentClient.call` or `SessionHandle.invoke` with a canonical operation
identity to let the catalog select the source or family route; process-scoped
operations may be called without a handle.
Lossless scalar constructors (`wireU64`, `wireI64`, `wireBytes`,
`wireUtf8Path`, `wireDuration`, `wireTimestamp`) are exported from the same
wire module and reject non-canonical or out-of-range values.
Language-local helpers are exported from the SDK root as well: `splitUtf8Chunks`
and `IncrementalUtf8Decoder` preserve UTF-8 byte boundaries across streamed
reads, while `cleanJson` and `extractJsonFromMarkdown` match the Rust JSON
parsing utilities for trailing commas and fenced markdown output.
`ToolCallParams` preserves the Rust parameter accessors and required-type
validation. `ToolResult` is available as a factory value (`success`,
`successJson`, `failure`, `invalidArguments`) with immutable `with*` modifiers;
its structured data is encoded through the same lossless WireValue helpers.
Failure categories are closed and preserve Rust recovery (`restore_then_retry`
for unavailable, `verify_then_retry` for timeout/partial side effects,
`retry` for transient and `stop` otherwise); ordinary JSON objects are always
encoded as maps even when they contain `kind`/`value` keys.
The A2A `TaskState` union and `taskStateCanTransitionTo` helper preserve the
closed terminal and transition table without adding a wire route.
`A2AMessage`, `A2ATaskStatus`, `AgentProvider` and `AgentSkill` provide the
same immutable text/status/provider/skill value constructors.
`AgentCard.builder(...)` provides the immutable card value and fluent local
builder; it does not synthesize a card from a Host-owned Agent.
`SessionHandle.updates()` and `RunHandle.events` are bounded async iterables;
Run events validate stream identity and sequence, acknowledge consumed cursors,
surface gaps, and fail with `host_exited` if the Host exits unexpectedly.
Settled `RunHandle` instances also expose `status()`, `outcomeStatus()`, and
`usage()`. The usage counters (`duration_ms`, `tokens_used`, and `iterations`)
remain textual `WireU64` values, preserving integers that exceed JavaScript's
safe number range.
`EchoAgentClient.classifyTurnOutcome(eventWire)` delegates terminal-event
classification to the Rust authority and returns its explicit Variant or Null
wire value without reimplementing outcome rules in TypeScript.

Typed `ToolDescriptor`, `LlmClientDescriptor`, `StoreDescriptor` and
`CriticDescriptor` descriptors are accepted by `registerTool`,
`registerLlmClient`, `registerStore` and `registerCritic`;
`registerContextCompressor` accepts a named compressor descriptor. Its call
includes a temporary tokenizer reference; `EchoAgentClient.countTokens`
executes the exact Host tokenizer for arbitrary callback text.
Handlers receive the Host-issued `ExtensionInvokeCall` with its operation,
extension handle, invocation identity, deadline and optional stream handle.
Typed results must use the matching operation discriminator; streaming calls
return a Host-minted stream outcome. These helpers only adapt the existing
`_echo_agent/extension/*` contract and do not create a second execution path.
Critic handlers receive the Host-issued `critic_critique` call (`task`, `answer`, and
`context`) and return the structured `Critique` result (`score`, `passed`,
`feedback`, and `suggestions`). Verifier policy, retries, and run settlement
remain Host-owned.
Context-compressor handlers receive the typed message set, lossless token limit,
optional focus fields and return the retained/evicted messages plus an optional
checkpoint. Cancellation, timeout and Session teardown use the same Host-owned
extension invocation authority.
`registerAgentComponent` exposes the closed component/operation contract for
Host-consumed conversation/run/runtime state, audit, context projection,
memory trigger, guard, search, workflow/checkpoint, revisioned task, sandbox,
MCP transport, embedder and memory-promoter implementations. Calls/results use
operation-discriminated unions with per-variant runtime validation rather than
a raw payload. The Host validates the nested discriminators and remains the
Agent lifecycle authority. Conversation ensure/search, Run append/parent-list,
IntentClassifier, SkillLoadPolicy, cancel-aware sandbox execution and
Workflow/Sandbox stream variants are explicit union members. Streaming
callbacks return the Host stream handle and use `agentComponentChunk` /
`agentComponentComplete`; separate chunk and terminal unions prevent invalid
inner terminal sequencing.
