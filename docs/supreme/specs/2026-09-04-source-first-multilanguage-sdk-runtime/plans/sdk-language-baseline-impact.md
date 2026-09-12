# Source Language SDK Baseline Impact

## SDK-Docs-Impact

`update`: the source-only TypeScript, Python and Java client baselines, shared
facade catalog, source build instructions, real-Host smoke checks and CI gate
are now delivered. `docs/sdk/`, the repository READMEs and ADR 0028 describe
the new baseline and explicitly keep the `Runnable` and `Parity complete`
claims closed until the full extension and all-feature matrix is verified.

The follow-up wire-boundary change also keeps signed/unsigned 64-bit values
and extension stream sequences inside the Rust contract range in all three
clients; it does not change the public status claims.

Java now exposes the same negotiated required-feature/capability checks and
session update stream observation as the TypeScript and Python clients.

The cross-language smoke configuration enables the documented framework memory
store explicitly so canonical `memory.store.put/get` checks exercise the Rust
authority instead of being skipped by a disabled optional setting.

The SDK clients now resolve every catalog-addressable operation through one
canonical call helper, selecting the source-operation or family route and its
catalog signature without duplicating dispatch semantics.

Session convenience methods use the same resolver, passing Agent plus Session
for source receivers and the Session handle directly for family receivers;
real Host smoke coverage exercises both routes.

Each language now exposes kind-specific extension registration helpers for all
eleven contract kinds. They delegate to the single generic registration RPC,
enforce descriptor version 1 and reject kind mismatches before transport.

Reverse extension handlers now normalize the closed result/stream/error
outcome union and convert cancellation races into a typed `cancelled` result;
each client exposes a stream-writer factory for Host-issued stream handles.

TypeScript notification queues now create their stream/update buffer on first
incoming notification, preventing early ACP events from being silently lost
before a consumer obtains the iterator.

All three SDK wire decoders now preserve typed and unknown variant
discriminators instead of returning only their payload, so additive values
remain observable and round-trip losslessly.

TS, Python and Java expose canonical constructors for signed/unsigned
integers, bytes, absolute UTF-8 paths, durations and timestamps; each helper
enforces the same Rust range and canonical-text rules before serialization.

Dedicated TypeScript and Python tests now exercise every scalar helper shape;
Java coverage remains in the shared JsonSupport test suite, and the real Host
gate continues to run after these unit checks.

Runtime compatibility is now explicit and machine-readable: Node.js 20+,
Python 3.10+ and JDK 17. This documents caller prerequisites only and does not
add runtime installation or bundled artifacts.

The parallel TS/Python/Java wire review independently verified the same
handle-kind, generation, integer, map-key and tagged-variant boundaries; the
language gate runs all three implementations against one source-built Host.

The three catalogs now expose deterministic enumeration of all 1,516 canonical
source/family operations (1,357 source and 159 family routes), with exact
signature/family/method assertions against the shared generated catalog.

Four ReactAgent setters whose receivers are process-local `Arc` authorities
are now classified by the existing language-local intrinsic rule instead of
being advertised as remotely callable source operations; generated parity and
catalog artifacts reflect the reduced executable surface.

The parallel lifecycle pass now gives each language an idiomatic bounded
subscription surface for ACP updates and facade events, Host-exit failure
propagation, stream identity/sequence checks, cursor acknowledgement/gap
handling, and bounded Run/Session/Client close behavior. Rust remains the
sole lifecycle authority.

The three real-Host smoke clients now verify asynchronous Run start, bounded
wait settlement and the final completed snapshot; tests do not infer terminal
state from the immediate start response.

Family calls now accept an absent handle for process-scoped operations such as
`telemetry.status`; Session-bound families still receive their Host-issued
Session handle and remain validated by the Host authority. Smoke coverage
exercises both forms.

The three SDKs now expose typed Tool, LlmClient and Store extension descriptors,
reverse invocation views, and operation-discriminated outcomes. These are thin
adapters over the existing `_echo_agent/extension/*` contract: Host-issued
identity, deadline, stream handles and cancellation remain visible, while Rust
continues to own execution, lifecycle and settlement. Generic registration APIs
remain available for extension kinds and payloads that do not have a typed
helper yet.

Reverse callback exceptions are normalized to the closed extension error-code
union (`extension_failed` unless the callback raised an existing typed SDK
code), so a user callback cannot emit the local transport-only error name and
break Host-side `ExtensionInvokeOutcome` decoding.

Plan 08 incremental parity work now routes the concrete ReactAgent steering
identities through the existing RunSteer authority, cascades resource-owned
facade streams through the unified HandleRegistry, and reports a runtime
unavailable media tool with its canonical operation identity. These are local
source/Host fixes with focused Rust evidence; at that intermediate point Plan
08 remained open because the design revision binding needed reconciliation and
most source-operation, consumer-trait, and production stream routes still
lacked parity evidence.

The RAG tool family now owns its embedder and vector index per ACP Session;
session close and connection teardown drop that state, while missing embedding
configuration fails with a typed error before an index/search tool is exposed.
Unit coverage proves same-session index/search and cross-session isolation.

The source-route inventory now records a small, exact set of local helper
identities (path/config, JSON parsing/canonicalization, UTF-8 chunking,
retention transforms and serde parsing) as `language-local-pure-helper`.
Filesystem utilities, retry closure execution, OS clock/timezone helpers and
other side-effectful authorities remain source-operation gaps; no broad
`utils` prefix was classified as intrinsic.

The Critic consumer trait now has a typed reverse bridge in Rust and all three
source SDKs. The bridge preserves `task`, `answer`, `context`, bounded
`score`, `passed`, `feedback` and `suggestions`; it reuses the existing
invocation lease/deadline/cancellation authority and never enables verifier
policy implicitly. Rust remains responsible for verify/retry/fail-open and run
settlement semantics.

The next source-operation slice adds read-only SkillRegistry projections through
the live Session Agent authority (`count`, descriptor listing and installed
checks), with strict Session receiver/argument validation. It deliberately
does not create a SkillRegistry handle or expose loader/path/hook/tool
construction seams as remote operations.

The follow-up projection keeps read-only code-skill listing, descriptor lookup,
catalog prompt, active sandbox policy, dependency tree and allowed-tool views
on that same Session Agent authority. Opaque loader, hook and tool-constructor
surfaces remain outside the wire contract.

Safe SkillRegistry source mutations (`tag_source`, source unregister and
descriptor removal) now reuse the same Session Agent write authority with
typed argument validation and real Host E2E coverage; opaque descriptor
registration and file-backed activation remain separate follow-up work.

The three source SDKs now also expose local helper equivalents for the exact
UTF-8 streaming and JSON parsing intrinsic routes. Their tests preserve Rust's
byte-capped scalar boundaries, malformed-byte replacement, pending suffix
flush, BOM behavior and quoted trailing-comma rules; these helpers do not add
wire operations or a second Host authority.

## Multilingual route-baseline closeout

The language mapping generator marks executable catalog routes `done` for all
three SDKs and keeps process-local intrinsic items explicitly
`not_implemented`. Source/family routes use the shared catalog resolver and
wire values use the lossless WireValue algebra; no second execution or state
authority is introduced. Rust inventory tests and the
TypeScript/Python/Java suites assert the executable mapping, and
`scripts/check-language-sdks.sh` validates all catalog identities/signatures
against a source-built `sdk-facade-all` Host. TypeScript and Python now also
ship executable `examples/quickstart` sources; the gate compiles and runs
both examples, while Java's existing `Example.java` remains in the same
chain. Full intrinsic behavior and Parity complete remain future work, so
`echo-website` stays unchanged.

## Intrinsic tool-value slice

`update`: the first language-local intrinsic slice is now executable in all
three source SDKs. TypeScript `ToolCallParams`/`ToolResult` uses immutable
factory/modifier values, Python exposes typed getters plus immutable
`ToolResult` factories, and Java adds equivalent typed parameter and result
helpers over Jackson. Cross-language tests cover successful construction,
structured data, required-type failures and every one of the 28 canonical
`intrinsic:language-local-wire-helper` manifest entries. This does not close
the remaining process-local, builder, cancellation or pure-algorithm intrinsic
routes, so overall Runnable/Parity complete and the website outcome remain
closed. Failure recovery tags and optional failure fields are validated as a
closed lossless DTO, and ordinary JSON objects are force-encoded as maps so
`kind`/`value` field names cannot be confused with pre-encoded WireValue data.

## A2A TaskState intrinsic slice

`update`: TypeScript, Python and Java now expose the closed A2A `TaskState`
terminal/transition table and display values in their native enum/union forms.
All 10 canonical identities (six variants, enum, two behavior methods and the
Display implementation) have focused tests and `done` mappings. No wire or
second state authority is introduced; the remaining intrinsic routes stay
open.

## A2A value intrinsic slice

`update`: Message, TaskStatus, Provider and Skill now have immutable native
constructors and projections in all three SDKs. The 14 canonical `a2a_values`
identities are covered by behavior and mapping tests; no network, Host or
second lifecycle authority is introduced.

The follow-on Agent Card slice adds the local fluent builder and immutable card
projection in all three SDKs. Its 14 canonical identities are covered by
focused behavior/mapping tests; `from_agent` stays explicitly unimplemented
because the card must be derived by the Rust Agent authority.

The A2A Artifact/Error DTO slice adds immutable wire-field projections in all
three SDKs; its eight canonical identities use a dedicated behavior mapping.

The A2A stream value slice adds typed status/artifact events and response
wrappers in all three SDKs; its 18 canonical identities remain transport-free.

The task envelope slice adds immutable request/params/task/response DTOs in all
three SDKs; its 15 canonical identities preserve nested history and artifact
values without taking execution authority.

The ThinkingLevel slice adds the seven reasoning levels and case-insensitive
Rust parser aliases in all three SDKs; its nine canonical identities use
dedicated behavior evidence.

The steering value slice adds accepted/drained/settled state and typed turn
outcomes without projecting the live receipt handle; its 13 canonical
identities use dedicated behavior evidence.

The Subagent value slice adds durable command phases and runtime statuses
without projecting dispatch or message receipt ownership; its 15 canonical
identities use dedicated behavior evidence.

The content-guard value slice adds six decision/operation identities while
reusing existing PII payload field contracts and keeping guard execution in
Rust/Host.

The GuardResult slice uses the language-native `GuardDecision` name to avoid
colliding with Python's existing component result helper; its six canonical
identities preserve pass/block/warn/transform payloads.

The delivery value slice adds 16 outcome/phase identities and stable snake-case
spellings without projecting the durable delivery ledger authority.

The Subagent stop value slice adds six hook status identities without projecting
hook execution or dispatch authority.

The task terminal value slice adds seven PlanTask terminal status identities
without projecting task execution authority.

The permission rule source slice adds ten source/parse/display identities,
preserving source-priority aliases without projecting rule evaluation.

The permission rule behavior slice adds six behavior/parse/decision identities,
preserving allow/deny/ask payloads without projecting RuleRegistry authority.

The permission mode helper slice adds six parse/display/predicate identities,
preserving mode policy semantics without projecting evaluation authority.

The permission rule matcher slice adds nine parse/display/matching identities
without projecting RuleRegistry or evaluation authority.

The command-cell phase slice adds ten phase/string/terminal identities without
projecting process or sandbox execution authority.

The command-cell status slice adds 15 terminal-cause/artifact-status identities
without projecting process or artifact-writer execution authority.

The Team strategy slice adds seven strategy/name/description identities without
projecting Team dispatch or coordination authority.

The ACP runtime value slice adds 13 connection-mode, extension-settlement and
ledger-limit identities. The SDKs preserve the local values and settlement
predicate while ACP connection/session and event-ledger authority remains in
Rust/Host.

The ACP adapter configuration slice adds 13 metadata, resource-limit,
shutdown-duration and validation identities. It remains a local validated
snapshot; adapter construction, connection admission and shutdown authority
remain in Rust/Host.

The ACP lease-error slice adds five enum/display identities for typed admission,
concurrency and exclusive-conflict failures. The SDKs preserve the display
contract while lease admission and concurrency authority remain in Rust/Host.

The A2A JWT value slice adds nine configuration/debug/subject identities. The
SDKs preserve immutable local configuration and claims access while key
verification, token validation, and A2A server ownership remain in Rust/Host.

The dependency/source value slice adds seven `DepKind` and `SkillSource`
identities. The SDKs preserve stable local spellings while dependency probing,
skill loading, and source policy remain in Rust/Host.

The context-inheritance slice adds 11 default/field identities. The SDKs
preserve inheritance fields and mode presets while Subagent context, memory,
history and dispatch authority remain in Rust/Host.

The observed-isolation slice adds four value/default/string identities. The
SDKs preserve trim, empty-default and Unicode-safe 512-scalar bounds while
isolation provider execution remains in Rust/Host.

The segment-range slice adds three struct/length/emptiness identities. The
SDKs preserve half-open saturating range semantics while message-cache state
remains in Rust/Host.

The prompt-diagnostics slice adds three section/record/count identities. The
SDKs preserve local diagnostic aggregation while prompt compilation remains in
Rust/Host.

The Subagent command-identity slice adds six command/attempt/validation
identities. The SDKs preserve durable identity constraints while live-control
registry and dispatch authority remain in Rust/Host.

The Subagent usage slice adds three cumulative-counter/payload identities. The
SDKs preserve sticky usage reporting while provider execution remains in
Rust/Host.

The artifact-config slice adds seven constructor/default/builder identities
(including generated source aliases). The SDKs preserve retention and limit
semantics while artifact writing remains in Rust/Host.

The Skill-validation slice adds two report/is-valid identities. The SDKs
preserve violation gating while Skill validation and loading remain in Rust/Host.

The Skill-content slice adds two struct/render identities. The SDKs preserve
prompt-block formatting while resource loading and execution remain in Rust/Host.

The MCP JSON-RPC slice adds four request/notification constructor identities.
The SDKs preserve the local `2.0` value shape while MCP transport remains in
Rust/Host.

The HookAction slice adds 30 tagged variant/field/validation identities. The
SDKs preserve configuration validation while command, HTTP, MCP and Subagent
hook execution remains in Rust/Host.

The PageInfo slice adds two metadata/apply identities. The SDKs preserve
truncation and continuation projection while pagination state remains in
Rust/Host.

The Subagent context slice adds three empty/content identities. The SDKs
preserve local snapshot semantics while tools, messages, stores and dispatch
remain in Rust/Host.

The Usage slice adds six provider-normalized cache/effective-token identities.
The HookEvent slice adds 45 stable event/category/ordering/classification
identities. TypeScript, Python and Java preserve the Rust event names, category
partition, parser, tool-event and matcher predicates as local values; hook
dispatch and matcher execution remain Host/framework-owned.
The EventIdentity slice adds 29 event/stream identity constructor, accessor,
validation and immutable-update identities. UUID-backed run/chat factories and
runtime-context projection remain local value behavior; event sequencing and
stream ownership remain Host-side.
The InterventionResult slice adds seven local decision identities for allow,
block, cancel, inject and argument modification. All three languages preserve
the immutable decision fields while callback dispatch and cancellation remain
Host-owned.
The TokenBudget/LlmTimeouts slice adds 35 local policy identities. Percentage
allocation, compression excess/reporting and zero-as-disabled timeout
semantics are preserved in each language without moving model execution or
timeout ownership out of Rust.
The ExecutionUsage slice adds the duration helper identity, preserving the
absent-to-zero projection across all three languages without taking over run
accounting.
The TurnMode slice adds three chat/execute mode identities, preserving the
stream flavor locally while the turn driver remains Host/framework-owned.
The RetryPolicy slice adds nine local construction/backoff identities,
preserving no-retry/default policies, exponential caps and jitter configuration
without moving retry execution out of Rust.
The ThinkingConfig slice adds 14 local variant/parser/provider-projection
identities, preserving disable/level/budget semantics and provider-specific
effort mappings without moving provider transport out of Rust.
The platform time slice adds eight clock and RFC3339 helper identities,
preserving Unix timestamps, local-offset formatting, UTC instant round-trips
and optional null handling without persisted runtime state.
The ThinkingProtocol slice adds 13 provider-dialect and field-emission
identities, preserving local configuration semantics without moving provider
transport out of Rust.
The sandbox ResourceLimits slice adds four default/strict/unrestricted policy
identities, preserving resource caps and path lists without creating sandbox
processes or taking execution ownership.
The ProviderCapabilities slice adds four provider default/name-resolution
identities, preserving protocol feature snapshots without moving provider
transport out of Rust.
The ThinkingProfile slice adds five model/provider protocol-resolution
identities, preserving manual level selection without contacting provider
transports.
The ModelProfile slice adds thirty-two provider/model policy identities,
preserving context limits, tokenizer selection, and provider/exact override
precedence without moving model policy transport out of Rust.
The SDKs preserve calculation priority while LLM execution remains in Rust/Host.

The HookAction slice adds 31 tagged variant/field/validation identities. The
SDKs preserve configuration validation while command, HTTP, MCP and Subagent
hook execution remains in Rust/Host.

The Subagent usage slice adds three cumulative-counter/payload identities. The
SDKs preserve sticky usage reporting while provider execution remains in
Rust/Host.

The Subagent command-identity slice adds six command/attempt/validation
identities. The SDKs preserve durable identity constraints while live-control
registry and dispatch authority remain in Rust/Host.

## SDK-Skill-Impact

`none`: this stage adds language SDK source clients and protocol/catalog
validation only. It does not add, remove or change any Agent Skill, Skill
discovery path, skill contract, or skill execution authority.
