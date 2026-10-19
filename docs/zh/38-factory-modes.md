# Agent Factory 与模型能力 Profile

## 概述

echo-agent 只有一套通用执行引擎：`ReactAgent`。应用通过 builder 配置、工具、提示词、
skills 和 invocation policy 组装 agent。框架不存在 `ModeEngine`、`AgentMode`，也不存在内置的
Coding/Research/Data/Writing 运行时状态机。

embedding application Chat/Task/Auto 等产品模式属于应用层。应用可以按模式选择提示词和 invocation 工具面，
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

`ModelProfileResolver` 是 framework 合成动态模型事实的唯一权威。每个注册的
`ModelFactSet` 都携带 source、provenance、version、观测/过期时间和有界 confidence。
解析按字段执行，优先级从低到高为：

1. conservative unknown；
2. versioned built-in catalog；
3. fresh provider facts；
4. fresh exact-model facts；
5. fresh exact caller override。

过期或晚于解析时刻的记录进入 `ModelProfileResolution::ignored_stale_facts`，不能开启
structured output、工具、并行调用或更大的 token budget。未知 `LlmClient` 实现同样默认
使用保守能力。

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

Provider adapter 或应用发现精确模型事实时使用 `register_exact_model_facts`。
`register_provider_default`、`register_exact` 和三参数 `resolve` 继续作为兼容入口，但会立即
转换为带 provenance 的记录并进入同一 resolver。

`ModelFactInputs` 是既有 `LlmConfig` 与 `ModelConfig` 的可序列化 sidecar。
`SourcedLlmConfig` 和 `SourcedModelConfig` 保留 provider、exact-model 与有序 caller records，
同时不改变原公开 config struct 形状。旧的 `ModelConfig::context_window` 会转换为带来源的 caller
override。具体 client 通过 `protocol_capabilities` 发布 adapter 固有的 wire baseline，可配置
provider 名称不能改变该协议事实；原有 `capabilities` 方法返回 freshness 解析后的 profile，
而不是 wire baseline。

通过 `ReactAgentBuilder::sourced_llm_config` 安装 LLM sidecar。每个 run snapshot 都会重新解析当前 client，并保留
`model_fact_sources`、`ignored_model_facts`，因此过期记录不能继续隐藏在 budget、compression、thinking
或工具策略中。tokenizer fact 目前只作为 receipt 元数据记录，runtime 仍使用 calibrated heuristic
estimator；provider tokenizer dispatch 留给 #100 后续边界。

通过 `ReactAgentBuilder::model_profile(profile)` 安装解析结果。工具排除项会并入不可变的
EffectiveRunPolicy；prompt suffix 会进入 canonical system context，压缩后仍可恢复。run 进入
FinalOnly 后，支持 `tool_choice=none` 的 provider 会收到显式控制；不支持的 provider 使用空工具面
加 final-answer prompt 回退。

resolver 不内置大规模模型表。快速变化的模型事实应由消费应用或 provider integration
携带有界过期时间注册。参见 [ADR 0047](../adr/0047-model-facts-freshness-authority.md)。

### 切换已安装模型

`ReactAgent::set_model(name)`、`set_llm_config(config)` 和
`set_sourced_llm_config(config)` 返回 `Result<()>`。切换模型先构造新的 concrete client，
成功后一起发布模型名、保留配置与新 profile。同一个 Agent 的下一次 invocation 使用新
client；已开始的 run 保留自己的 snapshot。endpoint、凭据、协议、显式采样参数与显式
context limit 继续由调用方决定；隐式预算在下一次 prepare safe point 跟随新模型刷新。

`set_llm_client`、`with_llm_client` 安装 opaque 自定义 client 时清除旧的重建配置。

随后切换到其它模型需要显式提供替换 client 或配置，否则 `set_model` 返回现有
`ConfigError::MissingConfig`，不修改当前状态。设置已经一致的模型是 no-op；空模型名与
client 构造失败也不发布任何字段。`with_*llm_config` builder 保持返回 `Self`，安装失败
仅记录错误；需要接收构造错误时使用 fallible setter 或 `ReactAgentBuilder::build`。

Prepared model generation 在 prepare 中解析并冻结模型事实，commit 只发布已准备好的
值；丢弃 receipt 保留旧 generation。`AgentHandle` 转换出的 boxed Agent 按既有合同
保留转换时的字符串快照；查询当前模型应读取同一 live handle。

工厂已经构造好 client 时，可调用
`ReactAgent::new_with_configured_llm_client(agent_config, client, original_config)`。
它保留同一个 Arc 和原始重建配置，只查询一次 profile 后走普通 Context 初始化，
不重建 client，也不将隐式预算变成显式。caller 必须提供构造 client 的原已校验配置；
任意 client 只能检查非空且相同的模型身份，无法证明 endpoint/凭据关联。
无效 pair 返回既有 typed 配置错误，不回显配置或凭据。应在应用 custom extensions
之前构造，后续 opaque client 注入仍清除配置；这个 Rust 构造入口不新增 SDK 远程 API。

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
