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
`ToolCallParams` provides the Rust parameter getters and required-type checks;
`ToolResult` adds immutable static factories (`success`, `successJson`,
`failure`, `invalidArguments`) and `with*` modifiers over the existing JSON
shape.
Failure categories are closed and preserve Rust recovery actions; ordinary
JSON objects passed to `successJson`/`withData` are recursively encoded as
maps, while the `JsonNode` overload remains the explicit pre-encoded path.
`TaskState` is a Java enum with the same terminal, transition and display
semantics as the Rust A2A value; it does not add a protocol route.
`A2AMessage`, `A2ATaskStatus`, `AgentProvider` and `AgentSkill` provide the
corresponding immutable value constructors and text projections.
`AgentCard.builder(...)` provides the local immutable card and fluent builder;
it does not mirror the Host-owned `from_agent` operation.
`A2AArtifact.newArtifact(...)` and `A2AError.newError(...)` preserve the A2A
artifact/error wire fields as immutable values.
`TaskStatusUpdateEvent`, `TaskArtifactUpdateEvent` and `A2AStreamResponse`
provide the corresponding closed local stream values.
`A2ATaskParams`, `A2ATaskRequest`, `A2ATask`, and `A2ATaskResponse` provide
immutable nested task envelope values; task execution remains Host-owned.
`ThinkingLevel.parse(...)` preserves the Rust reasoning-effort aliases.
`AgentSteerState` and `AgentSteerTurnOutcome` expose the same local lifecycle
values while the active receipt remains Host-owned.
`SubagentCommandPhase` and `SubagentStatus` expose stable phase/status values;
dispatch ownership remains in the Host.
`ContentGuardResult` exposes pass/detect/reject/redact values while guard
execution remains Host-owned.
`GuardDecision` exposes Rust pass/block/warn/transform outcomes as immutable
sealed values.
`DeliveryOutcome` and `DeliveryPhase` expose stable ledger values while the
durable lifecycle remains framework-owned.
`SubagentStopStatus` exposes stable hook terminal values while hook execution
remains framework-owned.
`TaskTerminalStatus` exposes stable task terminal values while task execution
remains framework-owned.
`RuleSource` exposes permission source priority values and alias parsing while
rule evaluation remains framework-owned.
`RuleBehavior` exposes allow/deny/ask payloads and decision conversion while
rule evaluation remains framework-owned.
`PermissionMode` exposes aliases and policy helpers while mode evaluation
remains framework-owned.
`RuleMatcher` exposes pure tool/pattern/permission/all matching semantics while
registry evaluation remains framework-owned.
`CommandCellPhase` exposes stable phases and terminal classification while
command execution remains framework-owned.
Command-cell terminal causes and artifact statuses expose stable values while
execution remains framework-owned.
`TeamStrategy` exposes manager/pipeline/debate/swarm values while Team dispatch
remains framework-owned.
`AcpValues.ConnectionMode`, `AcpValues.ExtensionSettlement`, and
`AcpValues.AcpLedgerLimits` expose ACP runtime values while connection and
event-ledger ownership remain in the Host/Rust runtime.
`AcpAdapterConfig.defaults()` and `validate()` preserve adapter limits and
shutdown-duration validation without constructing the ACP adapter.
`AcpValues.ExtensionLeaseError` preserves typed lease failure text while the
Host retains admission and concurrency authority.
`JwtConfig` and `JwtClaims` preserve local A2A auth configuration and subject
projection while JWT verification remains Host/Rust-owned.
`DependencyKind` and `SkillSource` expose dependency/source values while
probing and skill loading remain framework-owned.
`ContextInheritance` exposes sync/fresh/fork/teammate/team defaults while
Subagent context and dispatch remain framework-owned.
`ObservedIsolation` exposes trim, empty-default, and code-point-safe bounds
while isolation execution remains framework-owned.
`SegmentRange` exposes half-open saturating length and emptiness while cache
state remains framework-owned.
`PromptDiagnostics` exposes section recording and per-id counts while prompt
compilation remains framework-owned.
`SubagentCommandIdentity` preserves durable ID validation and attempt
projection while live-control registry state remains framework-owned.
`LlmUsageStats` preserves cumulative token counters and payload projection while
provider execution remains framework-owned.
`ToolOutputArtifactConfig` preserves retention, threshold, and max-age builders
while artifact writing remains framework-owned.
`SkillValidationReport` preserves violation gating while validation remains
framework-owned.
`SkillContent` preserves structured prompt-block rendering while resource
loading and execution remain framework-owned.
`JsonRpcValues` preserves MCP `2.0` request/notification constructors while
transport remains framework-owned.
`HookAction` preserves tagged configuration and validation while hook
execution remains framework-owned.
`HookEvent` and `HookEventCategory` preserve stable names, `ALL` ordering,
category classification, parsing and matcher predicates while hook dispatch
remains framework-owned.
`EventId`, `StreamId` and `EventIdentity` preserve non-empty validation,
run/chat constructors, correlation fields and immutable `with*` updates.
`InterventionResult` exposes immutable allow/block/cancel/inject/argument
modification factories without owning callback execution.
`TokenBudget`, `TokenBudgetConfig`, `TokenAllocation` and `LlmTimeouts` expose
the same allocation, compression and zero-disabled timeout policy helpers.
`ExecutionUsage.durationMillis()` maps absent duration to zero without owning
run accounting.
`TurnMode` preserves the chat/execute stream flavor without owning the turn
driver.
`RetryPolicy` preserves default/no-retry factories, exponential backoff caps
and optional jitter configuration without running retries.
`ThinkingConfig` preserves disabled/level/budget variants, parsing and
provider effort/budget projections without owning LLM transport.
`TimeValues` preserves Unix timestamps, local-offset formatting, instant
round-trips and optional null values without persisted clock state.
`ThinkingProtocol` preserves provider dialect names and field-emission
semantics without owning provider transport.
`PageInfo` preserves truncation, continuation metadata, and output projection
while collection state remains framework-owned.
`SubagentContext` exposes empty/content semantics while context and dispatch
state remain framework-owned.
`Usage` preserves provider-normalized cache priority and effective token
calculations while LLM execution remains framework-owned.
`HookAction` preserves tagged configuration and validation while hook
execution remains framework-owned.
`LlmUsageStats` preserves cumulative token counters and payload projection while
provider execution remains framework-owned.
`SubagentCommandIdentity` preserves durable ID validation and attempt
projection while live-control registry state remains framework-owned.
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
