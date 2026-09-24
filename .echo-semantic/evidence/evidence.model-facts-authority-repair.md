---
schema_version: 1
id: evidence.model-facts-authority-repair
kind: evidence
observed_at: source:6ed43c02230186db2c60d15eeda68864a0c1dddb9719434c45439363536592e4
source_refs:
  - echo-core/src/llm/capabilities.rs
  - echo-core/src/llm/mod.rs
  - echo-integration/src/providers/config.rs
  - echo-integration/src/providers/mod.rs
  - echo-integration/src/providers/openai.rs
  - echo-integration/src/providers/responses.rs
  - echo-integration/src/providers/anthropic.rs
  - src/config.rs
  - src/lib.rs
  - src/llm.rs
  - src/agent/config.rs
  - src/agent/react/builder.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/phases/prepare.rs
  - src/agent/snapshot.rs
  - docs/adr/0047-model-facts-freshness-authority.md
supports: [behavior.llm-provider-execution, rule.provider-protocol-boundary]
limitations:
  - provider元数据获取和持久化仍由provider adapter或应用负责，framework只解析typed facts
  - tokenizer fact 只保留在 resolution receipt；当前 runtime 仍使用 calibrated heuristic tokenizer，provider dispatch 留给 #100
---

# Model facts 与 provider capability authority 修复证据

## 支持的结论

`ModelProfileResolver`现在是provider/model事实的唯一合成点。每个注册记录携带source、provenance、version、observed/expires时间和有界confidence；registration scope强制规范化source。解析顺序固定为conservative unknown、fresh versioned built-in、fresh provider、不可变adapter protocol、fresh exact model、fresh ordered caller override。过期、未来观测和空记录都不会提升结构化输出、Tool、并行调用、thinking或token预算，并在receipt中区分applied、ignored与原始registered records。Anthropic adapter 的真实能力不受自定义 provider label 覆盖；历史 OpenAI-compatible provider labels 保持显式兼容，未知 label 不按模型名称激活 catalog facts。

`ProviderCapabilityOverride`把adapter wire事实与动态model能力拆成partial字段，防止provider名称改写Chat/Responses/Anthropic adapter协议。`LlmClient::capabilities`默认保守；旧自定义client的显式override被包装为provider fact，内建client则从同一fresh resolution返回能力。`ModelProfile`旧borrowed字段、`LlmConfig`、`ModelConfig`和`RuntimeConfig::from_agent_config`的既有构造路径保持可用；`ModelProfileOverride`新增的可选字段要求 struct literal 使用 `..Default::default()`。`ModelFactInputs`、`SourcedLlmConfig`、`SourcedModelConfig`与新增builder/setter作为 additive sidecar。

每次run snapshot从client重新解析全部来源；无 live client 时也会从 retained registered facts 按当前时间重新解析，再逐字段重放仍fresh的 provider/exact/caller records，不复用冻结的旧profile。合并时 fresh facts 优先，旧 caller facts 只补 fresh 缺失字段；provider identity 为空的 unknown client 不能清除显式 provider profile。刷新后的context window、max output cap同时重建RuntimeConfig token limit/max_tokens与TokenBudget；prepare safe point在读取模型上下文前原子更新live ContextManager。当前 framework slice 不修改已从主线移除的 SDK Host，不引入第二套 resolver。

## 来源与范围

ADR 0047记录DU-68分层、行业依据、兼容与回滚；core拥有fact contract/resolver，integration provider只提供protocol facts，root config/builder/snapshot负责无损传递与run safe point；本 framework slice 不接管已移除的 SDK Host 边界。

## 已知缺口

SDK public inventory与多语言分类在并行集成分支统一生成；本切片不实现provider网络刷新任务、catalog下载、tokenizer calibration反馈或strict structured-output执行。
