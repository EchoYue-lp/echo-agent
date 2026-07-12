# Agent Factory 与模型能力 Profile

## 概述

echo-agent 只有一套通用执行引擎：`ReactAgent`。应用通过 builder 配置、工具、提示词、
skills 和 invocation policy 组装 agent。框架不存在 `ModeEngine`、`AgentMode`，也不存在内置的
Coding/Research/Data/Writing 运行时状态机。

EKO Chat/Task/Auto 等产品模式属于应用层。应用可以按模式选择提示词和 invocation 工具面，
但不应把产品模式写成模型能力或框架运行状态。

## Agent Factory

`AgentFactoryConfig` 保存模型、名称、system prompt 和自定义工具所有权。
`echo_agent::agent::default_factory::DefaultAgentFactory` 消费该配置，并通过
`ReactAgentBuilder` 创建 `ReactAgent`。

```rust,no_run
use echo_agent::agent::default_factory::DefaultAgentFactory;
use echo_agent::agent::factory::{AgentFactory, AgentFactoryConfig};

# fn build() -> echo_agent::error::Result<()> {
let config = AgentFactoryConfig::new()
    .model("gpt-5")
    .name("assistant")
    .with_system_prompt("你是一个务实的编码助手。");

let factory = DefaultAgentFactory;
let agent = factory.create_agent(config)?;
# Ok(())
# }
```

直接构造 agent 时优先使用 `ReactAgentBuilder`。它提供完整框架配置面，是新代码的权威 API。

## Provider Capabilities 与 ModelProfile

`ProviderCapabilities` 描述 streaming tool delta、结构化输出、并行工具调用、显式
`tool_choice=none` 等协议能力。provider adapter 继续负责请求协议和 wire format 翻译。

`ModelProfile` 在 provider 能力之上补充模型级信息：

- thinking protocol 与 reasoning 支持；
- 图片、工具、streaming、并行工具能力；
- 已知 context/output 上限与 tokenizer；
- harness 工具排除项；
- 稳定 prompt suffix；
- 显式 `tool_choice=none` 支持。

应用可以通过 `ModelProfileResolver` 注册 provider 默认覆盖和规范化的 `provider:model`
精确覆盖。精确项优先。

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
            prompt_suffix: Some("工具参数保持简洁。".to_string()),
            ..Default::default()
        },
    );

let profile = resolver.resolve("ollama", "local-coder", ProviderCapabilities::ollama());
```

通过 `ReactAgentBuilder::model_profile(profile)` 安装解析结果。工具排除项会并入不可变的
EffectiveRunPolicy；prompt suffix 会进入 canonical system context，压缩后仍可恢复。run 进入
FinalOnly 后，支持 `tool_choice=none` 的 provider 会收到显式控制；不支持的 provider 使用空工具面
加 final-answer prompt 回退。

resolver 不内置大规模模型表。快速变化的模型事实应由消费应用或 provider integration 注册。

## Prompt Templates

`PromptTemplateManager` 提供命名模板注册和变量替换，与模型能力、产品模式相互独立。

```rust
use echo_agent::agent::PromptTemplateManager;

let mut templates = PromptTemplateManager::new();
templates.register("review", "评审 {{path}} 的正确性。");
let prompt = templates.render("review", &[("path", "src/lib.rs")])?;
# Ok::<(), String>(())
```

可复用提示文本使用 template；只有会改变 harness 行为的模型事实才进入 `ModelProfile`；产品工作流
模式继续留在应用层。
