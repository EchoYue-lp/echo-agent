---
schema_version: 1
artifact: delivery-map
design_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md
outcomes:
  acp-sdk-contract:
    ships: 一套覆盖根 echo_agent 全 feature 公共面的确定性 inventory、ACP relationship parity
      manifest、锁定官方稳定 ACP v1 基线的 _echo_agent/* 扩展 Schema/fixtures 与阻断漂移检查；不实现
      ACP Agent adapter、Host 或语言 SDK。
    depends_on: []
  acp-agent-adapter:
    ships: 根 echo_agent 的可选 acp feature、复用官方 Rust SDK 的通用 ACP Agent adapter，以及对稳定 v1
      initialize/session/prompt/update/cancel 与协商能力的 conformance 证据。
    depends_on:
      - acp-sdk-contract
  acp-standard-host:
    ships: 可从源码构建的 echo-agent-sdk-host，以产品无关默认 Agent 配置提供标准 ACP v1
      profile，标准客户端可完成会话、提示、更新、取消和有界关闭。
    depends_on:
      - acp-agent-adapter
  sdk-core-profile:
    ships: 在同一 ACP Host 与 Session/Run 权威上协商 _echo_agent/* core profile，提供
      Agent/Session/Run handle、完整 EventEnvelope、查询、取消、replay、gap、恢复和关闭语义。
    depends_on:
      - acp-standard-host
  sdk-extension-bridge:
    ships: 一条 namespaced 双向 extension bridge，使
      Tool、LlmClient、Store、HumanLoopProvider、Hook/Callback 与自定义 Agent 在超时、取消、断连和
      generation 竞争下保持 Rust 语义。
    depends_on:
      - sdk-core-profile
  facade-feature-adapters:
    ships: SDK Host 为根 echo_agent facade 的
      Task、Subagent、workflow、state、delivery、trace、eval、improve、MCP、A2A及全部
      feature 提供无重复权威的完整适配。
    depends_on:
      - sdk-core-profile
      - sdk-extension-bridge
  typescript-sdk:
    ships: 可从源码构建的 TypeScript ACP Client SDK，以 Promise、AsyncIterable、AbortSignal
      和语言惯用 handle/extension API 覆盖完整 facade，并通过真实 Host 验收。
    depends_on:
      - facade-feature-adapters
  python-sdk:
    ships: 可从源码构建的 Python ACP Client SDK，以 coroutine、AsyncIterator、取消作用域和 async
      context manager 覆盖完整 facade，并通过真实 Host 验收。
    depends_on:
      - facade-feature-adapters
  java-sdk:
    ships: 可从源码构建的 Java ACP Client SDK，以
      CompletionStage、Flow.Publisher、AutoCloseable 和语言惯用 extension API 覆盖完整
      facade，并通过真实 Host 验收。
    depends_on:
      - facade-feature-adapters
  sdk-docs-examples:
    ships: docs/sdk 唯一外部入口、标准 ACP 与完整 SDK 双 profile说明、三语言源码构建/兼容文档、可执行 quickstart 和全
      facade 等价示例，不宣称发布任何预编译或 registry 产物。
    depends_on:
      - acp-standard-host
      - typescript-sdk
      - python-sdk
      - java-sdk
  sdk-parity-closeout:
    ships: 相互独立的稳定 ACP v1 conformance 与三语言全 facade parity 门禁，以及全部
      feature、故障恢复、背压、进程清理和干净源码构建证据；全部通过后才标记 ACP conformant 与 Parity complete。
    depends_on:
      - acp-agent-adapter
      - acp-standard-host
      - sdk-core-profile
      - sdk-extension-bridge
      - facade-feature-adapters
      - typescript-sdk
      - python-sdk
      - java-sdk
      - sdk-docs-examples
  website-sdk-entry:
    ships: echo-website 在 ACP conformant 与 Parity complete 证据基础上提供准确的源码 SDK
      入口和构建说明，不宣传不存在的二进制或 registry 发布。
    depends_on:
      - sdk-parity-closeout
design_revision: sha256:9685cabd8f692ea060b67e46fe1c7d47531edb44efd998913e252803ef122817
---
# 交付图说明

本主题先冻结官方稳定ACP v1基线、根facade关系清单和echo-agent扩展合同，再交付通用ACP Agent adapter与标准Host。完整SDK core profile、extension bridge和全facade适配共享同一执行权威；TypeScript、Python、Java随后独立交付，最终分别关闭ACP conformance与SDK parity门禁，官网最后接入。

计划编号只表示创建顺序。每个outcome只有在依赖已由代码和验证证据实际交付后才能进入Build。
