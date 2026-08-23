# Runtime Configuration

`echo_agent` accepts typed values and never searches for an application config
file. Products own file formats, precedence, secrets, model catalogs, prompts,
channels, and UI/server settings.

## FrameworkConfig

`FrameworkConfig` is a serializable adapter for one Agent runtime. It contains
only provider-neutral model settings and Agent behavior settings.

```rust
use echo_agent::config::{AgentYamlConfig, FrameworkConfig, ModelConfig};
use echo_agent::llm::LlmApiProtocol;

let config = FrameworkConfig {
    model: ModelConfig {
        provider: "compatible".into(),
        name: "my-model".into(),
        api_protocol: Some(LlmApiProtocol::ChatCompletions),
        ..Default::default()
    },
    agent: AgentYamlConfig {
        name: "assistant".into(),
        system_prompt: "Help the user.".into(),
        ..Default::default()
    },
};
let agent_config = config.to_agent_config();
```

The framework default does not enable tools, memory, persistence, or
human-in-the-loop behavior. Applications opt into those policies explicitly.

## Explicit Paths

Use `DataRoot` as a value and pass concrete paths to stores and services:

```rust
use echo_agent::paths::DataRoot;

let root = DataRoot::new("/var/lib/my-agent");
let memory_path = root.path("memory.json");
```

The framework does not select a home-directory name and has no process-global
data-root setter.

## PermissionMode

`AgentConfig::permission_mode` and `ReactAgent::set_permission_mode` accept the
typed `PermissionMode` enum. String aliases and product-specific display names
belong at the application boundary.

## Tool Composition

`StandardToolPack` is installed explicitly by `ReactAgent`. Read-only Agents
receive the capability-filtered projection of the same pack; applications may
provide additional `ToolPack` implementations or register individual tools.

## Application Configuration

The embedding application application's YAML schema, model catalog, channel settings, TUI/server
settings, and product prompt are documented in the `echo-agent-cli` repository.
