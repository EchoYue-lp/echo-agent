# Runtime Configuration

`echo_agent` accepts typed values and never searches for an application config
file. Products own file formats, precedence, secrets, model catalogs, prompts,
channels, and UI/server settings.

## FrameworkConfig

`FrameworkConfig` is the serializable runtime configuration for one Agent. It
contains only provider-neutral model settings and Agent behavior settings.

```rust
use echo_agent::agent::AgentConfig;
use echo_agent::config::{AgentSettings, FrameworkConfig, ModelConfig};
use echo_agent::llm::LlmApiProtocol;

let config = FrameworkConfig {
    model: ModelConfig {
        provider: "compatible".into(),
        name: "my-model".into(),
        api_protocol: Some(LlmApiProtocol::ChatCompletions),
        ..Default::default()
    },
    agent: AgentSettings {
        name: "assistant".into(),
        system_prompt: "Help the user.".into(),
        ..Default::default()
    },
};
let agent_config: AgentConfig = config.into();
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
typed `PermissionMode` enum. The framework owns its canonical kebab-case ids
and accepted aliases; applications can use `str::parse` or serde directly
without a parallel mode DTO.

Provider configuration views also expose the framework `LlmApiProtocol` and
`ModelInputModality` values directly. An embedding application should preserve
those typed enums and only choose a separate wire name when its transport
contract genuinely differs.

## LLM Timeouts

`LlmConfig::with_timeouts(LlmTimeouts)` sets the provider-client default for
complete requests and stream first-chunk, idle, and overall boundaries.
`ChatRequest::with_timeouts` overrides the same value for one call. Public code
uses `Duration`; serialized `LlmTimeouts` values are optional milliseconds.
See [Streaming Output](./10-streaming.md#llm-timeouts) for defaults and exact
boundary semantics.

## Tool Composition

`StandardToolPack` is installed explicitly by `ReactAgent`. Read-only Agents
receive the capability-filtered projection of the same pack; applications may
provide additional `ToolPack` implementations or register individual tools.

## Application Configuration

The embedding application application's YAML schema, model catalog, channel settings, TUI/server
settings, and product prompt are documented in the `echo-agent-cli` repository.
