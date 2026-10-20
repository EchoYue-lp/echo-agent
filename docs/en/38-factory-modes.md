# Agent Factory and Model Profiles

## Overview

echo-agent has one general execution engine: `ReactAgent`. Applications shape an
agent with builder configuration, tools, prompts, skills, and invocation policy.
The framework does not provide a `ModeEngine`, `AgentMode`, or built-in
Coding/Research/Data/Writing mode state machine.

Product modes such as embedding application Chat/Task/Auto belong in the application layer. They may
choose prompts and invocation-scoped tools, but they should not become model
capabilities or framework runtime states.

## Agent Factory

`AgentFactoryConfig` captures the model, name, system prompt, and owned custom tools.
`echo_agent::agent::default_factory::DefaultAgentFactory` consumes that value and
builds a `ReactAgent` through `ReactAgentBuilder`.

```rust,no_run
use echo_agent::agent::default_factory::DefaultAgentFactory;
use echo_agent::agent::factory::{AgentFactory, AgentFactoryConfig};

# fn build() -> echo_agent::error::Result<()> {
let config = AgentFactoryConfig::new()
    .model("gpt-5")
    .name("assistant")
    .with_system_prompt("You are a practical coding assistant.");

let factory = DefaultAgentFactory;
let agent = factory.create_agent(config)?;
# Ok(())
# }
```

For direct construction, prefer `ReactAgentBuilder`. It exposes the complete
framework configuration surface and is the canonical API for new code.

## Provider Capabilities and Model Profiles

`ProviderCapabilities` describes protocol behavior such as streaming tool deltas,
structured output, parallel tool calls, and explicit `tool_choice=none` support.
Provider adapters remain responsible for wire-format translation.

`ModelProfile` combines those protocol capabilities with model-specific information:

- thinking protocol and reasoning support;
- image, tool, streaming, and parallel-tool support;
- known context/output limits and tokenizer;
- harness tool exclusions;
- a stable prompt suffix;
- explicit `tool_choice=none` support.

`ModelProfileResolver` is the framework authority for combining changing model
facts. Every registered `ModelFactSet` carries source, provenance, version,
observation/expiration timestamps, and bounded confidence. Resolution is field-wise:

1. conservative unknown;
2. versioned built-in catalog;
3. fresh provider facts;
4. fresh exact-model facts;
5. fresh exact caller override.

Expired and future-observed records are returned in
`ModelProfileResolution::ignored_stale_facts` and cannot enable structured output,
tools, parallel calls, or larger token budgets. Unknown `LlmClient`
implementations likewise default to conservative capabilities.

```rust
use echo_agent::llm::{
    ModelFactConfidence, ModelFactMetadata, ModelFactSet, ModelFactSource,
    ModelProfileOverride, ModelProfileResolver, ProviderCapabilities,
};
use std::time::{Duration, SystemTime};

let observed_at = SystemTime::now();
let resolver = ModelProfileResolver::new()
    .register_provider_facts(
        "ollama",
        ModelFactSet::new(
            ModelFactMetadata::new(
                ModelFactSource::ProviderAdapter,
                "ollama:/api/show",
                "manifest-digest-v1",
                observed_at,
                observed_at.checked_add(Duration::from_secs(300)),
                ModelFactConfidence::from_percent_saturating(90),
            ),
            Some(ProviderCapabilities::ollama()),
            ModelProfileOverride::default(),
        ),
    )
    .register_explicit_override(
        "ollama",
        "local-coder",
        ModelFactSet::new(
            ModelFactMetadata::new(
                ModelFactSource::CallerOverride,
                "application:model.local-coder",
                "config-v3",
                observed_at,
                None,
                ModelFactConfidence::VERIFIED,
            ),
            None,
            ModelProfileOverride {
                context_window: Some(32_768),
                ..Default::default()
            },
        ),
    );

let resolution = resolver.resolve_at("ollama", "local-coder", observed_at);
let profile = resolution.profile;
```

Provider adapters or applications that discover exact-model data use
`register_exact_model_facts`. `register_provider_default`, `register_exact`, and the
three-argument `resolve` remain compatibility entry points; they immediately
translate their values into sourced records and use the same resolver.

`ModelFactInputs` is the serializable sidecar for existing `LlmConfig` and
`ModelConfig` values. `SourcedLlmConfig` and `SourcedModelConfig` preserve provider,
exact-model, and ordered caller records without changing the existing public config
struct shapes. A legacy `ModelConfig::context_window` is converted to a sourced
caller override. Concrete clients publish an adapter-specific
`protocol_capabilities` baseline; configurable provider labels cannot change that
wire contract. Their existing `capabilities` method returns the fresh resolved
profile instead of the protocol baseline.

Use `ReactAgentBuilder::sourced_llm_config` for the LLM sidecar. Every run snapshot resolves
the current client again and retains `model_fact_sources` and `ignored_model_facts`, so an
expired record cannot remain hidden in budget, compression, thinking, or tool policy. The
tokenizer fact is receipt metadata only; the current runtime still uses its calibrated
heuristic estimator. Provider tokenizer dispatch remains a follow-up for #100.

Install the resolved value with `ReactAgentBuilder::model_profile(profile)`. Tool
exclusions join the immutable effective tool policy. The prompt suffix becomes part
of canonical system context and survives compression. When a run enters final-only
mode, providers that support `tool_choice=none` receive it explicitly; other
providers receive an empty tool surface plus the final-answer instruction.

The resolver intentionally ships without a large model catalog. Fast-changing model
facts should be supplied by the consuming application or provider integration with
a bounded expiration time. See [ADR 0047](../adr/0047-model-facts-freshness-authority.md).

### Changing an installed model

`ReactAgent::set_model(name)`, `set_llm_config(config)` and
`set_sourced_llm_config(config)` return `Result<()>`. A successful model change
rebuilds the configured concrete client before publishing its model, retained
configuration and fresh profile together. The same Agent's next invocation uses
that client; an existing run retains its own snapshot. Endpoint, credentials,
protocol, explicit sampling and explicit context limits remain caller choices.
An implicit context budget follows the new model at the next prepare safe point.

Calling `set_llm_client` or `with_llm_client` installs an opaque custom client and
clears any previous rebuild configuration. Switching it to a different model
requires an explicit replacement client or configuration; `set_model` otherwise
returns `ConfigError::MissingConfig` without changing state. Setting the already
consistent model is a no-op. Empty names and failed client construction likewise
leave the installed state unchanged. The `with_*llm_config` builders retain their
`Self` result and log installation failures; use a fallible setter or
`ReactAgentBuilder::build` when construction errors must be returned.

Prepared model generation resolves and freezes model facts during preparation.
Commit only publishes prepared values, and dropping the receipt keeps the old
generation. A boxed Agent created from an `AgentHandle` retains its documented
conversion-time string snapshots; read the live handle for the current model.

Factories that already built a client can use
`ReactAgent::new_with_configured_llm_client(agent_config, client, original_config)`.
It retains the same Arc and the original rebuild configuration, resolves the
profile once before ordinary Context initialization, and preserves implicit
versus explicit budget choices. It does not rebuild the client. Supply the
original validated configuration: only nonempty matching model identity can be
checked for arbitrary clients, not endpoint or credential provenance. Invalid
pairs return a typed configuration error without including supplied secrets.
Apply custom extensions afterward; later opaque client injection clears this
configuration. This Rust construction entry does not introduce a remote SDK API.

Use `ReactAgentBuilder::configured_llm_client(client, original_config)` when the
factory also needs the full Builder injection path, including a shared Subagent
registry, tools and stores. It uses the same identity validation, single profile
preparation and frozen publisher as direct construction; no client is rebuilt.
`build` and `build_strict` report invalid pairs before constructing Agent-owned
subsystems. The pair's model remains authoritative over a later `model(...)`.

The explicit pair replaces earlier client/config inputs. A later `llm_client`,
`llm_config` or `sourced_llm_config` replaces the pair, so an opaque replacement
does not inherit its rebuild configuration. Without this new entry, ordinary
client/config combinations retain their existing behavior: config construction
wins over client injection regardless of setter order. Later runtime opaque
injection still clears rebuild configuration. `demo26_provider_factory` compiles
and exercises this construction path without making model requests.

## Prompt Templates

`PromptTemplateManager` performs named template registration and variable
substitution. It is independent from model capabilities and product modes.

```rust
use echo_agent::agent::PromptTemplateManager;

let mut templates = PromptTemplateManager::new();
templates.register("review", "Review {{path}} for correctness.");
let prompt = templates.render("review", &[("path", "src/lib.rs")])?;
# Ok::<(), String>(())
```

Use templates for reusable prompt text. Use `ModelProfile` only for facts that alter
harness behavior, and keep application workflow modes in the application.
