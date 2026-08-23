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

Applications can register a provider default and a more specific normalized
`provider:model` override with `ModelProfileResolver`. The exact entry wins.

```rust
use echo_agent::llm::{ModelProfileOverride, ModelProfileResolver, ProviderCapabilities};
use std::collections::HashSet;

let resolver = ModelProfileResolver::new()
    .register_provider_default(
        "ollama",
        ModelProfileOverride {
            supports_parallel_tool_calls: Some(false),
            supports_tool_choice_none: Some(false),
            ..Default::default()
        },
    )
    .register_exact(
        "ollama",
        "local-coder",
        ModelProfileOverride {
            excluded_tools: HashSet::from(["browser".to_string()]),
            prompt_suffix: Some("Use compact tool arguments.".to_string()),
            ..Default::default()
        },
    );

let profile = resolver.resolve("ollama", "local-coder", ProviderCapabilities::ollama());
```

Install the resolved value with `ReactAgentBuilder::model_profile(profile)`. Tool
exclusions join the immutable effective tool policy. The prompt suffix becomes part
of canonical system context and survives compression. When a run enters final-only
mode, providers that support `tool_choice=none` receive it explicitly; other
providers receive an empty tool surface plus the final-answer instruction.

The resolver intentionally ships without a large model catalog. Fast-changing model
facts should be supplied by the consuming application or provider integration.

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
