# echo-agent Java SDK (source)

This is a source-only Java 17 ACP v1 client built on the official
`com.agentclientprotocol:acp-core:0.17.0` transport/session runtime. The Host executable is supplied
explicitly by the caller; this module does not bundle a JDK, Host binary or
JAR. The transport follows ACP's newline-delimited JSON-RPC framing and the
facade payloads are taken from the checked-in canonical catalog.
Build and run it with JDK 17.

```bash
mvn test
mvn -q package
java -cp target/echo-agent-sdk-source-0.1.0.jar:... \
  com.echoagent.sdk.Example /absolute/path/to/echo-agent-sdk-host \
  /absolute/path/to/host.json
```

`src/main/java/com/echoagent/sdk/Example.java` is the executable Java
quickstart and is compiled and run by the repository language gate.

`EchoAgentClient` exposes `CompletionStage` request methods, `Flow.Publisher`
session updates and run events, `AutoCloseable` Agent/Session/Run handles and typed
`EchoAgentException` failures. Jackson is used only for `_echo_agent/*`
extension values; standard ACP JSON-RPC/stdio lifecycle remains owned by the
official SDK. No Agent/Task/Subagent execution logic is reimplemented.
Typed `ToolDescriptor`, `LlmClientDescriptor`, `StoreDescriptor`, `CriticDescriptor` and
`ContextCompressorDescriptor`
builders emit the versioned registration snapshots from the shared extension contract.
Their `ToolCall`, `LlmChatCall`, `StoreCall` and `CriticCall` handlers expose Host-issued
identity, deadline and stream handles without hiding the generic `JsonNode` APIs.
`ToolOutcome`, `LlmOutcome`, `StoreOutcome`, `CriticOutcome` and `CompressionOutcome` builders serialize
operation-discriminated results through the existing `_echo_agent/extension/invoke`
callback; they do not create a second execution path. Critic callbacks receive the
Host-issued `critic_critique` input (`task`, `answer`, `context`) and return the structured
`Critique` value (`score`, `passed`, `feedback`, `suggestions`). Verifier policy,
retries and run settlement remain Host-owned; registering a Critic does not enable
verification implicitly.
`registerContextCompressor` supplies a typed `CompressionCall` and accepts a
`CompressionOutput`; invocation deadlines, cancellation and session teardown remain
owned by the same Host extension bridge. `CompressionCall.tokenizer()` plus
`EchoAgentClient.countTokens` use the exact Host-owned tokenizer.
`registerAgentComponent` uses `AgentComponentDescriptor`, `AgentComponentCall`
and `AgentComponentOutcome` for Host-consumed conversation/run/runtime state,
audit, context projection, memory trigger, guard, search,
workflow/checkpoint, revisioned task, sandbox, MCP transport, embedding and
memory promotion. Calls decode to the sealed `AgentComponentRequest` hierarchy,
and results use the sealed `AgentComponentResult` hierarchy; nested component
and operation discriminators remain checked by the Host.
The sealed hierarchies include conversation ensure/search, Run
append/parent-list, IntentClassifier, SkillLoadPolicy, cancel-aware sandbox
execution and Workflow/Sandbox stream variants. Streaming handlers return
`ExtensionOutcome.stream` and publish the separate sealed chunk/terminal types
with `agentComponentChunk` / `agentComponentComplete`.
`EchoAgentClient.call` and `SessionHandle.invoke` select source or family
operations from the shared canonical catalog; process-scoped operations accept
a null handle and Session-bound operations use the Host-issued Session handle.
`WireValues` provides bounded `u64`, `i64`, `bytes`, `utf8Path`, `duration`
and `timestamp` constructors for the extension scalar algebra.
`Utf8Support` provides language-local `splitUtf8Chunks`, `cleanJson` and
`extractJsonFromMarkdown` helpers. `IncrementalUtf8Decoder` incrementally decodes
byte streams without splitting UTF-8 scalars, replaces malformed bytes with
U+FFFD, and retains incomplete suffixes until `finish()`. These helpers are
pure Java utilities and do not add Host, ACP or catalog protocol surface.
`RunHandle.status()`, `RunHandle.outcomeStatus()` and `RunHandle.usage()` query
the Host-owned settled receipt through the canonical Run receiver. They return
`JsonNode` values directly; `WireU64` counters in usage remain textual nodes so
large values are not converted through Java numeric types.
`EchoAgentClient.classifyTurnOutcome(eventWire)` delegates terminal event
classification to the canonical `TurnOutcome::classify` source operation. Use
`WireValues.record` or `WireValues.variant` to construct typed structural event
values; the Java SDK does not duplicate the framework's classification rules.
Session updates and Run events use bounded `Flow.Publisher` instances; event
sequence gaps fail explicitly, consumed events are ACKed, and `closeAsync()`
provides bounded CompletionStage shutdown for sessions, runs and the client.
