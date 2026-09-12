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

## SDK-Skill-Impact

`none`: this stage adds language SDK source clients and protocol/catalog
validation only. It does not add, remove or change any Agent Skill, Skill
discovery path, skill contract, or skill execution authority.
