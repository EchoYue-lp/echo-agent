# Agent Factory, Mode Engine & Prompt Templates

## Overview

These three components work together to provide flexible Agent creation and configuration:

| Component | Responsibility | Module |
|-----------|---------------|--------|
| Agent Factory | Create Agent instances | `echo-core::agent::factory` + `echo_agent::agent::default_factory` |
| Mode Engine | Customize system prompts and tool recommendations per work mode | `echo-core::agent::mode` + `echo_agent::agent::mode_engine` |
| Prompt Templates | Dynamic prompt generation with variable substitution | `echo-core::agent::prompt_template` |

```
User request → AgentFactory (create Agent) → ModeEngine (choose mode) → PromptTemplate (render prompt) → Agent
```

---

## Agent Factory

### What It Is

Agent Factory implements the factory pattern for creating Agent instances. Callers do not need to know concrete Agent implementations — they provide a configuration and receive a configured Agent instance.

> **Architecture Note**: The framework uses a single Agent engine (ReactAgent) design, where different execution strategies are implemented through tools and configuration rather than separate Agent types. This aligns with industry-leading frameworks (Hermes, Claude Code, LangGraph, etc.).

### AgentFactoryConfig

`AgentFactoryConfig` captures all configuration needed to create an Agent:

| Field | Type | Description |
|-------|------|-------------|
| `mode` | `Option<AgentMode>` | Optional work mode (e.g., Coding, Research) |
| `model` | `String` | LLM model identifier (e.g., "qwen3-max") |
| `name` | `String` | Agent name (for logging and orchestration) |
| `system_prompt` | `String` | System prompt |
| `tools` | `Vec<Box<dyn Tool>>` | Custom tool list |

### AgentFactory Trait

```rust
pub trait AgentFactory: Send + Sync {
    fn create_agent(&self, config: AgentFactoryConfig) -> Result<Box<dyn Agent>>;
}
```

Any type implementing this trait can serve as an Agent factory. The framework provides `DefaultAgentFactory` as the standard implementation.

### DefaultAgentFactory

`DefaultAgentFactory` is the concrete factory implementation provided by the facade layer (`echo_agent`), built on `ReactAgentBuilder`:

| Configuration | Builder Configuration |
|--------------|----------------------|
| Default | `.enable_tools()` |

> **Note**: `echo-core` also defines a `DefaultAgentFactory`, but it is a stub that returns an error. For actual use, always use `echo_agent::agent::default_factory::DefaultAgentFactory`.

### Code Example

```rust
use echo_agent::agent::default_factory::DefaultAgentFactory;
use echo_agent::agent::factory::AgentFactoryConfig;
use echo_core::agent::factory::AgentFactory;

let factory = DefaultAgentFactory;

// Create a coding assistant
let config = AgentFactoryConfig::new()
    .model("qwen3-max")
    .name("coder")
    .with_system_prompt("You are a coding assistant")
    .with_mode(AgentMode::Coding);

let agent = factory.create_agent(config)?;
println!("Agent: {}, Model: {}", agent.name(), agent.model_name());

// Create a research assistant
let config = AgentFactoryConfig::new()
    .model("qwen3-max")
    .name("researcher")
    .with_mode(AgentMode::Research);

let agent = factory.create_agent(config)?;
```

### Extended Capabilities

ReactAgent implements different execution strategies through tools and configuration:

| Strategy | Implementation | Example |
|----------|---------------|---------|
| Task Planning | Register plan/create_task tools + `execute_with_planning()` | Complex multi-step tasks |
| Self-Review | Register ReviewTool + LlmCritic | High-quality output |
| Multi-Agent Collaboration | SubAgent system | Parallel task execution |

---

## Mode Engine

### What It Is

Mode Engine defines Agent work modes (e.g., Coding, Research, Data Analysis, Writing). Each mode carries a default system prompt template and a recommended tool list. Applications can retrieve these defaults via `ModeEngine` and override them as needed.

### AgentMode Enum

| Mode | Display Name | Icon | Description |
|------|-------------|------|-------------|
| `General` | General | 💬 | General-purpose assistant, no domain specialization |
| `Coding` | Coding | 💻 | Code reading, writing, debugging, refactoring |
| `Research` | Research | 🔬 | Academic paper search, analysis, literature review |
| `Data` | Data Analysis | 📊 | Data analysis, statistics, visualization |
| `Writing` | Writing | ✍️ | Writing, editing, document formatting |

### ModeConfig

The configuration structure returned by `ModeEngine` for each mode:

```rust
pub struct ModeConfig {
    pub system_prompt_template: String,  // System prompt template
    pub recommended_tools: Vec<String>,    // Recommended tool names (empty = no restriction)
    pub display_name: String,            // Display name
    pub icon: String,                    // UI icon/emoji
}
```

### ModeEngine Trait

```rust
pub trait ModeEngine: Send + Sync {
    fn mode_config(&self, mode: &AgentMode) -> ModeConfig;
    fn all_modes(&self) -> Vec<AgentMode>;
    fn system_prompt(&self, mode: &AgentMode) -> String;
    fn recommended_tools(&self, mode: &AgentMode) -> Vec<String>;
}
```

### DefaultModeEngine

`DefaultModeEngine` provides English-language default prompt templates. Recommended tool counts per mode:

| Mode | Recommended Tools | Key Tools |
|------|------------------|-----------|
| General | 0 (no restriction) | All registered tools |
| Coding | 7 | shell, file_read, file_write, file_list, file_delete, code_search, git |
| Research | 8 | arxiv_search, semantic_scholar_search, pdf_fetch, bibtex_generate, web_search, web_fetch, file_read, file_write |
| Data | 16 | file_read, read_data, data_stats, profile_data, filter_data, aggregate_data, generate_chart, sample_data, correlate_data, pivot_data, time_series, hypothesis_test, regression, missing_value_analysis, outlier_detection, consistency_check |
| Writing | 4 | file_read, file_write, web_search, web_fetch |

### LocalizedModeEngine

`LocalizedModeEngine` supports localized prompt overrides, falling back to `DefaultModeEngine` for modes without overrides:

```rust
use echo_agent::agent::mode_engine::LocalizedModeEngine;
use echo_core::agent::mode::{AgentMode, ModeEngine};

// Build a localized engine (prompts and display names provided by the application)
let engine = LocalizedModeEngine::new()
    .with_override(AgentMode::Coding, "You are a professional coding assistant…".into())
    .with_display_name(AgentMode::Coding, "Code".into());

let config = engine.mode_config(&AgentMode::Coding);
println!("Prompt: {}", config.system_prompt_template);  // Custom prompt
println!("Display: {}", config.display_name);           // "Code"
println!("Tools: {:?}", config.recommended_tools);      // 7 tools (inherited from defaults)

// English-only parsing (framework level)
assert_eq!(LocalizedModeEngine::parse_from_str("coding"), Some(AgentMode::Coding));
assert_eq!(LocalizedModeEngine::parse_from_str("code"), Some(AgentMode::Coding));
assert_eq!(LocalizedModeEngine::parse_from_str("research"), Some(AgentMode::Research));

// Localized aliases (e.g. Chinese) are the application's responsibility
// (see echo-agent-cli's modes module for an example)
```

### Mode Parsing

`AgentMode::from_name` supports English aliases:

```rust
AgentMode::from_name("general")   // Some(AgentMode::General)
AgentMode::from_name("coding")    // Some(AgentMode::Coding)
AgentMode::from_name("code")      // Some(AgentMode::Coding)
AgentMode::from_name("research")  // Some(AgentMode::Research)
AgentMode::from_name("data")      // Some(AgentMode::Data)
AgentMode::from_name("writing")   // Some(AgentMode::Writing)
```

### Code Example

```rust
use echo_core::agent::mode::{AgentMode, DefaultModeEngine, ModeEngine};

let engine = DefaultModeEngine;

// Get full config for Coding mode
let config = engine.mode_config(&AgentMode::Coding);
println!("Mode: {} {}", config.icon, config.display_name);
println!("Prompt: {}", config.system_prompt_template);
println!("Tools: {:?}", config.recommended_tools);

// Iterate all modes
for mode in engine.all_modes() {
    let config = engine.mode_config(&mode);
    println!("{} {}: {} recommended tools",
        config.icon,
        config.display_name,
        config.recommended_tools.len()
    );
}
```

---

## Prompt Templates

### What It Is

`PromptTemplateManager` is a centralized prompt template registry and rendering engine supporting variable substitution, default values, and conditional blocks. Templates use `{{variable_name}}` syntax and are thread-safe (uses `RwLock` internally).

### Template Syntax

| Syntax | Format | Description |
|--------|--------|-------------|
| Variable | `{{name}}` | Replaced with the provided value |
| Default value | `{{name:default}}` | Uses default when variable is not provided |
| Conditional | `{{#if var}}...{{#endif}}` | Includes block when variable is present and non-empty |
| Conditional + alternative | `{{#if var}}...{{#else}}...{{#endif}}` | Shows first block when variable is present, otherwise alternative |
| Nested | `{{#if a}}{{#if b}}...{{#endif}}{{#endif}}` | Supports arbitrary nesting depth |

### PromptTemplateManager API

| Method | Description |
|--------|-------------|
| `new()` | Create empty template manager |
| `with_default_mode_templates()` | Create pre-loaded with default mode templates |
| `register(name, template)` | Register a named template (overwrites duplicates) |
| `remove(name) -> bool` | Remove a template |
| `contains(name) -> bool` | Check if template exists |
| `template_names() -> Vec<String>` | List all template names |
| `render(name, variables) -> Result<String>` | Render template by name |
| `render_template(template, variables) -> String` | Render a template string directly |
| `render_or_raw(name, variables) -> Result<String>` | Render or return raw string (optimization for static templates) |
| `get_template(name) -> Option<String>` | Get raw template string |

### Code Examples

#### Basic Variable Substitution

```rust
use echo_core::agent::prompt_template::PromptTemplateManager;

let manager = PromptTemplateManager::new();
manager.register("greeting", "Hello, {{name}}! Welcome to {{project}}.");

let result = manager.render("greeting", &[
    ("name", "Alice"),
    ("project", "EchoAgent"),
]);
assert_eq!(result.unwrap(), "Hello, Alice! Welcome to EchoAgent.");
```

#### Default Values

```rust
manager.register("fallback", "Hello, {{name:Guest}}!");

// Uses default when name is not provided
let result = manager.render("fallback", &[]);
assert_eq!(result.unwrap(), "Hello, Guest!");

// Uses provided value when name is given
let result = manager.render("fallback", &[("name", "Bob")]);
assert_eq!(result.unwrap(), "Hello, Bob!");
```

#### Conditional Blocks

```rust
manager.register("detail",
    "Base info. {{#if detail}}Details: {{detail}}.{{#endif}} End."
);

// Shows block when detail is provided
let result = manager.render("detail", &[("detail", "important info")]);
assert_eq!(result.unwrap(), "Base info. Details: important info. End.");

// Hides block when detail is absent
let result = manager.render("detail", &[]);
assert_eq!(result.unwrap(), "Base info.  End.");
```

#### Conditional + Alternative

```rust
manager.register("level",
    "{{#if premium}}Premium features enabled.{{#else}}Standard features.{{#endif}}"
);

let result = manager.render("level", &[("premium", "true")]);
assert_eq!(result.unwrap(), "Premium features enabled.");

let result = manager.render("level", &[]);
assert_eq!(result.unwrap(), "Standard features.");
```

#### Nested Conditionals

```rust
manager.register("nested",
    "{{#if a}}A present. {{#if b}}B too.{{#else}}B missing.{{#endif}}{{#else}}A missing.{{#endif}}"
);

let result = manager.render("nested", &[("a", "yes"), ("b", "yes")]);
assert_eq!(result.unwrap(), "A present. B too.");

let result = manager.render("nested", &[("a", "yes")]);
assert_eq!(result.unwrap(), "A present. B missing.");
```

#### Direct Rendering (No Registration)

```rust
let manager = PromptTemplateManager::new();
let result = manager.render_template(
    "Hello {{who}}!",
    &[("who", "World")]
);
assert_eq!(result, "Hello World!");
```

#### Pre-loaded Mode Templates

```rust
let manager = PromptTemplateManager::with_default_mode_templates();

// All mode templates auto-registered
assert!(manager.contains("mode_general"));
assert!(manager.contains("mode_coding"));
assert!(manager.contains("mode_research"));
assert!(manager.contains("mode_data"));
assert!(manager.contains("mode_writing"));

// Render coding mode prompt
let prompt = manager.render("mode_coding", &[])?;
println!("{}", prompt);  // Full coding assistant system prompt
```

#### Thread-Safe Sharing

```rust
use std::sync::Arc;

let manager = Arc::new(PromptTemplateManager::new());
manager.register("shared", "Hello, {{name}}!");

let m1 = Arc::clone(&manager);
let m2 = Arc::clone(&manager);

let r1 = m1.render("shared", &[("name", "A")]).unwrap();
let r2 = m2.render("shared", &[("name", "B")]).unwrap();

assert_eq!(r1, "Hello, A!");
assert_eq!(r2, "Hello, B!");
```

---

## Three-Component Integration

### Complete Workflow

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Choose mode     → .with_mode(AgentMode::Coding)           │
│ 2. Choose model    → .model("qwen3-max")                     │
│ 3. Create Agent    → factory.create_agent(config)?           │
│                                                             │
│    Internal flow:                                            │
│    ┌────────────────────────────────────────────────────┐   │
│    │ ModeEngine.mode_config(Coding)                      │   │
│    │   → Get system prompt template                      │   │
│    │   → Get recommended tool list                       │   │
│    │                                                     │   │
│    │ PromptTemplateManager.render("mode_coding", vars)   │   │
│    │   → Render prompt with variable substitution        │   │
│    │                                                     │   │
│    │ ReactAgentBuilder                                   │   │
│    │   → .system_prompt(rendered_prompt)                 │   │
│    │   → .tools(recommended_tools)                       │   │
│    │   → .build_boxed()                                  │   │
│    └────────────────────────────────────────────────────┘   │
│                                                             │
│ 4. Return Box<dyn Agent>                                     │
└─────────────────────────────────────────────────────────────┘
```

### Integration Example

```rust
use echo_agent::agent::default_factory::DefaultAgentFactory;
use echo_agent::agent::factory::{AgentFactory, AgentFactoryConfig};
use echo_agent::agent::mode_engine::LocalizedModeEngine;
use echo_core::agent::mode::{AgentMode, ModeEngine};
use echo_core::agent::prompt_template::PromptTemplateManager;

// 1. Initialize components
let factory = DefaultAgentFactory;
let mode_engine = LocalizedModeEngine::new()
    .with_override(AgentMode::Coding, "You are a professional coding assistant…".into())
    .with_display_name(AgentMode::Coding, "Code".into());
let template_manager = PromptTemplateManager::with_default_mode_templates();

// 2. Get mode configuration
let mode = AgentMode::Coding;
let mode_config = mode_engine.mode_config(&mode);

// 3. Use template manager to render prompt (optional, demonstrates integration)
template_manager.register("custom_coding", &mode_config.system_prompt_template);
let system_prompt = template_manager.render("custom_coding", &[
    ("extra_instruction", "Prefer Rust language"),
])?;

// 4. Create Agent
let config = AgentFactoryConfig::new()
    .model("qwen3-max")
    .name("rust-coder")
    .with_mode(mode)
    .with_system_prompt(&mode_config.system_prompt_template);

let agent = factory.create_agent(config)?;
```

---

## Configuration Reference

### AgentMode Parsing (LocalizedModeEngine)

| Input String | Parsed Result |
|-------------|---------------|
| `"general"` / `"通用"` | `AgentMode::General` |
| `"coding"` / `"code"` / `"编程"` / `"代码"` | `AgentMode::Coding` |
| `"research"` / `"研究"` | `AgentMode::Research` |
| `"data"` / `"数据分析"` / `"数据"` | `AgentMode::Data` |
| `"writing"` / `"写作"` / `"写"` | `AgentMode::Writing` |

### AgentFactoryConfig Defaults

| Field | Default Value |
|-------|--------------|
| `mode` | `None` |
| `model` | `""` |
| `name` | `"assistant"` |
| `system_prompt` | `"You are a helpful assistant"` |
| `tools` | `[]` |

### Template Syntax Quick Reference

```
{{variable}}              → Variable substitution
{{variable:default}}      → Variable with default value
{{#if var}}...{{#endif}}  → Conditional block
{{#if var}}...{{#else}}...{{#endif}}  → Conditional + alternative
{{ name }}                → Whitespace auto-trimmed
```

---

## Extension Guide

### Custom ModeEngine

```rust
use echo_core::agent::mode::{AgentMode, ModeConfig, ModeEngine};

pub struct MyCustomModeEngine;

impl ModeEngine for MyCustomModeEngine {
    fn mode_config(&self, mode: &AgentMode) -> ModeConfig {
        match mode {
            AgentMode::Coding => ModeConfig {
                system_prompt_template: "You are my dedicated coding assistant, following PEP 8 style.".into(),
                recommended_tools: vec!["shell".into(), "file_read".into()],
                display_name: "Custom Coding".into(),
                icon: "🛠️".into(),
            },
            // Fall back to DefaultModeEngine for other modes
            _ => echo_core::agent::mode::DefaultModeEngine.mode_config(mode),
        }
    }
}
```

### Custom AgentFactory

```rust
use echo_core::agent::factory::{AgentFactory, AgentFactoryConfig};
use echo_agent::error::Result;
use echo_agent::agent::Agent;

pub struct MyAgentFactory;

impl AgentFactory for MyAgentFactory {
    fn create_agent(&self, config: AgentFactoryConfig) -> Result<Box<dyn Agent>> {
        // Custom creation logic
        // Can inject custom LLM clients, middleware, etc.
        todo!("Custom Agent creation logic")
    }
}
```
