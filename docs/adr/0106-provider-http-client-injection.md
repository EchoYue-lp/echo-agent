# ADR 0106：宿主 HTTP 客户端注入与产品身份

状态：采用。日期：2026-11-06。

## 背景

应用经 LlmConfig 工厂创建客户端，但既有工厂无法传入宿主已经配置的 reqwest::Client。部分兼容网关拒绝没有 User-Agent 的请求。产品身份属于宿主，框架不能写入 Eclip 或其它具体产品品牌。Chat 与 Responses 已有 with_client 构造入口；Anthropic 缺少工厂内部注入接缝。

## 参考与候选

本机 DeepSeek Harness 0.2.1-rc.5 的 dsh-llm attributionHeaders 在产品边界统一提供真实身份；reqwest 0.12 的 ClientBuilder::user_agent 提供标准传输机制。复用现有 Client 与工厂足以解决问题。

候选一是在所有框架请求中硬编码应用品牌，拒绝：污染产品边界。候选二新增模型配置字段和第二套应用协议分派，拒绝：增加持久化/API 负担或重复事实解析。候选三复用 with_client，在既有 LlmConfig/SourcedLlmConfig 工厂增加显式 HTTP 客户端注入，采用。

## 决策与影响

增加 build_client_with_http_client，原 build_client 委托同一 factory 且保留默认传输行为；不修改 LlmConfig 序列化结构、不新增依赖、不改变协议选择、模型事实优先级、鉴权或生命周期。Anthropic 仅增加内部 client 注入接缝。产品应用构造具有自身 User-Agent 的 reqwest::Client；GUI 连接测试、主 Agent 与 Subagent 经既有 prepare 主路径复用它。

注入客户端是宿主传输策略，不授予模型能力、不改变 provider/endpoint/model 身份、不添加权限门控。已有 examples 纳入编译/执行验证；消费者通过真实 loopback 验证三协议普通/流式请求头与完整结束。
