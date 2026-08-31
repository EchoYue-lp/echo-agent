# 运行时配置

`echo_agent` 只接收类型化配置，不搜索应用配置文件。文件格式、优先级、密钥、模型
目录、产品提示词、渠道和 UI/server 设置均由上层应用负责。

## FrameworkConfig

`FrameworkConfig` 是单个 Agent runtime 的可序列化配置值，只包含 provider-neutral
模型设置和 Agent 行为设置。

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
        system_prompt: "帮助用户。".into(),
        ..Default::default()
    },
};
let agent_config: AgentConfig = config.into();
```

框架默认不启用工具、记忆、持久化或 HITL；这些策略必须由应用显式选择。

## 显式路径

把 `DataRoot` 当作普通值，并将具体路径传给 store/service：

```rust
use echo_agent::paths::DataRoot;

let root = DataRoot::new("/var/lib/my-agent");
let memory_path = root.path("memory.json");
```

框架不选择 home 下的目录名，也不提供进程全局 data-root setter。

## PermissionMode

`AgentConfig::permission_mode` 与 `ReactAgent::set_permission_mode` 接收类型化
`PermissionMode`。framework 直接拥有 canonical kebab-case id 和可接受的字符串别名；
应用可以直接使用 `str::parse` 或 serde，不需要平行的 mode DTO。

Provider 配置 view 也直接暴露 framework `LlmApiProtocol` 和
`ModelInputModality`；应用应保留这些类型化 enum，只有在 transport 合同确实不同时才另设 wire 名称。

## LLM 超时

`LlmConfig::with_timeouts(LlmTimeouts)` 设置完整请求以及 stream first-chunk、idle、
overall 边界的 provider-client 默认值；`ChatRequest::with_timeouts` 使用同一个类型覆盖
单次调用。公开 API 使用 `Duration`，序列化的 `LlmTimeouts` 使用可选毫秒值。默认值与
精确边界见[流式输出](./10-streaming.md#llm-超时)。

## 工具组合

`ReactAgent` 显式安装 `StandardToolPack`。只读 Agent 使用同一工具包按 capability
推导的只读投影；应用也可以实现其它 `ToolPack` 或注册单独工具。

## 应用配置

embedding application 的 YAML schema、模型目录、渠道、TUI/server 设置和产品提示词位于
`echo-agent-cli` 仓库文档。
