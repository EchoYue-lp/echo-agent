# ADR 0047: Model Facts Freshness Authority

- Date: 2026-09-15
- Owners: `echo-core` LLM contract and provider adapter maintainers
- Upstream decision: ADR 0045 DU-68
- Findings: #68, framework portion of #77

## Status

Accepted.

## Context

`ProviderCapabilities`, `ModelProfile`, and `ModelProfileResolver` already form the
framework model-policy surface, but their inputs do not identify where a fact came
from, when it was observed, which source version supplied it, or when it expires.
The resolver also receives a bare capability value and `LlmClient` defaults unknown
implementations to an optimistic OpenAI-compatible capability set. A stale catalog
or an unclassified custom endpoint can therefore silently enable structured output,
tools, or parallel tool calls.

Current provider APIs do not expose a uniform metadata contract:

- the official [OpenAI Python SDK model object](https://github.com/openai/openai-python/blob/main/src/openai/types/model.py)
  exposes identity, creation, ownership, and optional shutdown date, but not a full
  capability profile;
- the official [Anthropic Python SDK model object](https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/model_info.py)
  exposes optional capabilities and token limits alongside release time;
- [LangChain model profiles](https://github.com/langchain-ai/langchain/blob/master/libs/core/langchain_core/language_models/model_profile.py)
  are generated from an external registry, include lifecycle/update fields, permit
  absent facts, and warn about version mismatch;
- the [LiteLLM model catalog](https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json)
  carries optional limits, capability flags, source links, and deprecation dates.

The common pattern is layered, incomplete, changing information. Provider metadata
can improve a local catalog but cannot replace a conservative fallback or an
application override.

## Decision

`ModelProfileResolver` remains the only framework authority that composes model and
provider facts. No second registry, provider-owned policy engine, or application
state store is introduced.

### Fact contract

Every registered fact set carries:

- `source`: caller override, exact model source, provider adapter, built-in catalog,
  or conservative unknown;
- `provenance`: a stable locator such as an endpoint, config key, document, or
  catalog name;
- `version`: the source revision, ETag, model revision, or catalog version;
- `observed_at` and optional `expires_at` timestamps;
- bounded `confidence` expressed as an integer percentage.

A fact set may carry protocol/provider capability overrides and a partial
`ModelProfile` override. Missing fields do not erase lower-precedence facts.
Registration normalizes `source` to its actual precedence layer, so a caller cannot
label provider facts as an exact override or make a receipt misrepresent authority.
Multiple caller records for the same exact identity are applied in registration
order. This lets a legacy explicit field remain fresh when a separate configured
override has expired, while preserving both records in the receipt.

### Resolution order and freshness

Resolution is field-wise, from lowest to highest precedence:

1. conservative unknown;
2. versioned built-in catalog;
3. fresh provider facts;
4. fresh exact-model facts;
5. fresh exact caller override.

The result exposes the metadata records that were applied and those rejected as
stale. A record is fresh only when `observed_at <= now` and `expires_at` is absent or
`now <= expires_at`. A future observation or an expiration earlier than the
observation is therefore ignored. The resolver accepts `now` explicitly in its
canonical API so tests and recovery paths are deterministic.

Stale records never contribute capability values. Unknown providers and the default
`LlmClient` implementation use a conservative capability set; concrete provider
adapters declare an immutable protocol baseline independent of the configurable
provider label. Historical OpenAI-compatible provider labels remain explicit
built-in aliases; an unclassified gateway does not inherit model-family facts by
matching only a model name. Model-dependent support is supplied through typed facts.
In particular, an unknown custom client does not inherit OpenAI structured-output or
tool capability merely because it implements `LlmClient` or uses an OpenAI-style
endpoint.

Existing `register_provider_default`, `register_exact`, and `resolve` methods remain
as compatibility entry points. They are translated immediately into the canonical
fact-set resolver with explicit synthetic provenance; they do not own separate
precedence or freshness logic.

### Built-in catalog

The small built-in catalog remains code-owned and deliberately conservative. Its
metadata has a stable catalog version and observation epoch. It is a fallback, not
proof that an external service still supports a feature. Applications or provider
integrations can supersede it with fresh records without changing framework code.

## Options Considered

### Let every provider or application resolve its own profile

Rejected because it preserves the current authority split. Different consumers
would disagree about precedence and stale handling.

### Fetch provider metadata inside `echo-core`

Rejected because core would acquire network, credential, retry, cache, and provider
lifecycle responsibilities. Provider adapters or applications already own those
boundaries and only need to submit typed facts.

### Treat built-in values as permanently authoritative

Rejected because model limits and feature support change independently of framework
releases. Version metadata without precedence and expiration is insufficient.

## Consequences

- Provider and application integrations can refresh facts without introducing a
  second model-policy authority.
- Resolution remains synchronous and side-effect free; fetching and persistence are
  outside this ADR.
- A resolved tokenizer id is receipt metadata only. The current runtime continues to
  use its calibrated heuristic tokenizer; provider tokenizer dispatch is tracked as
  follow-up work for #100.
- When a live client has no provider identity, it cannot erase an explicit caller
  profile for the same model. Same-layer fresh facts win; retained caller facts are
  replayed only for fields the fresh resolution does not provide.
- The public Rust API gains fact metadata, fact-set, confidence, and resolution
  receipt types. SDK facade artifacts may classify these language-local policy
  values during the integration branch's consolidated contract regeneration.
- #68 can close after implementation, focused tests, semantic verification, and
  independent rereview. #77 additionally requires all production capability
  consumers to use the resolved authority; this slice closes its framework default
  and concrete-adapter declaration prerequisite but does not claim the downstream
  migration is complete.

## Compatibility And Rollback

The existing `ModelProfile`, `ModelConfig`, `LlmConfig`, and
`RuntimeConfig::from_agent_config` construction paths remain available. The
resolver adds optional model-level fields to `ModelProfileOverride`; callers
using struct literals should use `..Default::default()` so those optional facts
remain forward-compatible. New `ModelFactInputs`, `SourcedModelConfig`, and
`SourcedLlmConfig` sidecars carry serialized fact inputs; additive builder/setter
methods accept them. Existing `context_window` values are converted to a
synthetic caller-override record. `AgentConfig` and `RuntimeConfig` retain the
complete resolution receipt so budget, compression, thinking, tool policy, trace,
and recovery observe the same provenance. A run snapshot refreshes retained facts at
its safe point even when no live `LlmClient` is attached.

A rollback can remove the optional fact inputs and restore the previous optimistic
`LlmClient` default without stored-data migration because older configurations do
not require the new fields and fact records are immutable caller inputs.

## Verification

Focused tests cover the complete precedence chain, source normalization,
fresh/expired/future-observed records, empty records, built-in historical time,
capability/profile invariants, a stale record that attempts to elevate structured
output/tools/budgets, conservative unknown providers, compatibility entry points,
serialized config round trips, and immutable protocol declarations on each concrete
provider adapter. Builder/snapshot tests prove that one retained receipt drives
budget, compression, thinking, and tool policy. Formatting, affected package tests,
and package-level Clippy complete the repair evidence.
