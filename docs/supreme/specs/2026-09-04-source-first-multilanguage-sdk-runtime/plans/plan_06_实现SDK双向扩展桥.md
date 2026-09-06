---
schema_version: 3
lifecycle: ready
supersedes: null
slug: 2026-09-04-source-first-multilanguage-sdk-runtime/plan
goal: 在已交付的 ACP core profile 上实现单一连接权威管理的双向 SDK extension bridge，使宿主语言注册的公开扩展点可被
  Rust Agent 无损调用。
ships: 一条 namespaced 双向 extension bridge，使
  Tool、LlmClient、Store、HumanLoopProvider、Hook/Callback 与自定义 Agent 在超时、取消、断连和
  generation 竞争下保持 Rust 语义。
verify: 源码构建的 Host 在协商 ExtensionBridge 后可完成
  Tool、LlmClient、Store、HumanLoopProvider、Hook、AgentCallback、InterventionCallback、AgentFactory
  与自定义 Agent 的注册、反向调用、流式结果、取消、超时、断连和注销；标准 ACP profile 保持不变，错误、背压、关闭和 generation
  竞争没有假成功；合同漂移检查、./scripts/verify.sh 与适用 feature 矩阵全部通过。
design_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md
delivery_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/plans/delivery-map.md#sdk-extension-bridge
todos:
  - id: freeze-extension-bridge-contract
    files:
      - echo-sdk-protocol/src/capability.rs
      - echo-sdk-protocol/src/error.rs
      - echo-sdk-protocol/src/methods.rs
      - echo-sdk-protocol/src/event.rs
      - echo-sdk-protocol/src/catalog.rs
      - echo-sdk-protocol/src/schema.rs
      - echo-sdk-protocol/tests/extension_contract.rs
      - contracts/sdk/schema/echo-agent-extension-v1.schema.json
      - contracts/sdk/fixtures/extension/v1/
      - contracts/sdk/source-contract.json
      - contracts/sdk/parity-manifest.json
    summary: 冻结双向注册、调用、取消和流式结果的 typed extension 合同。
    verify: 每个 ExtensionKind 的 descriptor、operation、error、stream 和 generation
      规则都能由官方 typed JSON-RPC 类型编解码，并由 Schema 与正反 fixture 覆盖。
  - id: build-shared-extension-runtime
    files:
      - src/acp/mod.rs
      - src/acp/runtime.rs
      - src/acp/adapter.rs
      - src/acp/extension.rs
      - tests/acp_extension_runtime.rs
    summary: 在 ACP 连接级建立唯一 extension invocation、取消和关闭权威。
    verify: 共享 runtime 能以唯一 invocation identity 管理并发、deadline、取消、late response 和
      teardown，并且标准 Prompt/Run runtime 继续复用同一连接。
  - id: integrate-host-extension-registry
    files:
      - echo-sdk-host/src/config.rs
      - echo-sdk-host/src/core_profile/mod.rs
      - echo-sdk-host/src/core_profile/state.rs
      - echo-sdk-host/src/core_profile/handles.rs
      - echo-sdk-host/src/core_profile/handler.rs
      - echo-sdk-host/src/core_profile/extension_bridge.rs
      - echo-sdk-host/src/factory.rs
    summary: 把协商后的 ExtensionBridge 接入 Host registry、generation handle 和 Session
      Agent factory。
    verify: Host 只在协商且实际编译支持时 advertisement ExtensionBridge；register/unregister
      具有连接所有权、generation fence、资源上限和事务回滚，且不产生第二个 Session/Run/Agent 权威。
  - id: implement-trait-proxies-and-streams
    files:
      - echo-sdk-host/src/core_profile/extension_bridge.rs
      - echo-sdk-host/src/core_profile/wire.rs
      - echo-sdk-host/src/core_profile/events.rs
      - echo-sdk-host/tests/extension_bridge_e2e.rs
    summary: 实现各公开 trait 的薄代理、反向 request dispatcher 和有界 stream 结果。
    verify: Tool、LlmClient、Store、HumanLoopProvider、Hook、AgentCallback、InterventionCallback、AgentFactory
      与自定义 Agent 均通过同一 bridge 成功返回、typed failure、stream terminal、timeout、cancel
      和 disconnect 结果。
  - id: prove-bridge-reliability-and-docs
    files:
      - echo-sdk-host/tests/extension_bridge_e2e.rs
      - echo-sdk-host/tests/core_profile_e2e.rs
      - echo-sdk-protocol/tests/core_rpc_contract.rs
      - docs/sdk/sdk-core-profile.md
      - docs/sdk/sdk-extension-bridge.md
      - docs/sdk/README.md
      - docs/sdk/protocol.md
      - docs/sdk/acp-agent-adapter.md
      - docs/adr/0028-source-first-multilanguage-sdk-runtime.md
      - README.md
      - README.zh.md
      - CHANGELOG.md
      - .github/workflows/rust-ci.yml
    summary: 用真实 Host/官方 Client 验收桥接故障矩阵并同步源码交付文档。
    verify: 真实 Host E2E 覆盖标准客户端不变、协商失败、generation、重入、backpressure、Host/SDK
      退出和密钥隔离；文档明确 bridge 已可用但三语言 SDK 仍未 Runnable。
artifact_id: plan:ed8047b7-0147-4232-b52b-c547f48f85f2
design_revision: sha256:9685cabd8f692ea060b67e46fe1c7d47531edb44efd998913e252803ef122817
---
## Approach

- 以 echo-agent 根 crate 的 AcpConnectionServices 为连接级生命周期边界，在 src/acp/extension.rs 增加 protocol-neutral 的 extension invocation authority。该 authority 只管理 registration identity、invocation lease、deadline、CancellationToken、并发许可、exactly-once settlement 和 close admission，不定义第二套 Agent、Session、Run、terminal、retry 或 recovery 语义。
- 由 echo-sdk-protocol 冻结每个 ExtensionKind 的 typed descriptor 和 operation envelope。公共请求、响应、通知实现官方 agent-client-protocol 的 JsonRpcRequest、JsonRpcResponse、JsonRpcNotification；WireValue 只承载已经由 kind 和 operation 选定的业务值，不允许 Host 通过任意 JSON 猜测 trait 语义。
- 由 echo-sdk-host 的 core profile 持有 connection-owned extension registry 和 generation-fenced extension handles。Host 在标准 ACP initialize 完成并且 Client hello 匹配后才开放 ExtensionBridge；每个注册记录绑定 implementation id、kind、descriptor snapshot、limits 和 connection ownership，Host 重启或连接关闭后不恢复客户端 callback。
- 反向调用统一使用官方 ConnectionTo<Client>::send_request、send_cancel_request、send_notification 与 Builder composition。Host 发送一个带 extension handle、invocation id、operation、session/run context 和 deadline 的 typed request；语言 SDK 的 dispatcher 在自己的受控 executor 中执行用户实现并返回 result、stream 或 typed error。
- trait proxy 只负责把 Rust trait 方法转换为 canonical extension operation，再把结果还原给既有 framework service。Tool、LlmClient、Store、HumanLoopProvider、Hook、AgentCallback、InterventionCallback、AgentFactory 和自定义 Agent 均复用同一 transport 和 invocation registry，不新增每种 trait 的平行协议。
- 流式 callback 以独立 Stream handle 和单调序列传递 chunk，必须有且只有一个 Complete、Failed 或 Cancelled terminal；响应、通知、取消、超时和断连都经过同一 invocation/generation fence，late response 只能被丢弃并记录有界诊断。

## Global Constraints

- 基础 wire 继续由官方稳定 ACP v1、agent-client-protocol 2.1.0 和 schema 1.7.0 定义；禁止 unstable_protocol_v2、draft schema、自建 JSON-RPC envelope、parser 或 stdout writer。
- 所有自定义方法必须位于 _echo_agent/* namespace 并由 initialize 的 _meta.echo_agent capability 声明。Plain Client、未协商 Client 和 capability 不匹配的 Client 不得创建或调用 extension handle；标准 ACP initialize/session/prompt 行为保持可用。
- AcpConnectionServices、SessionRegistry、AgentTurnDriver、TurnReceipt、EventEnvelope、RuntimeStateStore 和 EventJournal 仍是唯一权威。Host adapter 不重算终态、重试、DAG、ready frontier、事件序列或恢复事实。
- Extension handle 只在当前 connection generation 有效；shape、kind、generation、issued/closed 的校验顺序保持与 core profile 一致。注册、注销、invocation、stream 和 in-flight callback 数量、字节和等待时间全部有界。
- 每个 invocation 使用独立字符串 identity，不能借用 JSON-RPC request id；同一 identity 的 response 只能结算一次。取消与自然完成竞争时以 framework 的终态/CAS 语义为准。
- reader loop 不运行语言 callback；callback 必须有 deadline、CancellationToken 和受控并发许可。callback 不得重入同一个对象的排他 mutation，重入返回 typed conflict，而不是等待形成死锁。
- callback 失败、超时、取消、断连和 late response 不触发内置实现 fallback，除非对应 Rust API 已定义同样的显式 fallback 语义。未知 feature、descriptor、operation 和 payload 必须 fail closed。
- Connection teardown 按关闭 admission、取消 extension invocations、等待有界结算、flush profile state、关闭 Session Agent/MCP、释放 handles 的顺序执行；stdin EOF 与显式 close 共用该路径。
- 密钥不得进入 descriptor、事件、错误 details、stderr 或 stdout；stdout 只能承载官方 ACP wire。所有错误、payload、stream 和队列均需有界，Rust 生产代码不得使用 unwrap、expect、panic、unreachable、越界索引或 UTF-8 字节截断。
- InterventionCallback 是根 facade 的公开且可改变执行结果的 trait，本计划必须显式支持或给出 typed feature-unavailable；不得通过只支持 observational AgentCallback 的命名近似来静默遗漏它。内部和文档术语继续只使用 Subagent，不新增 Worker。
- 本计划只交付 Rust Host、协议合同、薄 proxy、测试和文档源码；不创建 npm、wheel、JAR、预编译 Host 或 Node/Python/JDK/JRE/Rust runtime，也不宣称 TypeScript、Python、Java 已 Runnable 或 Parity complete。

## Files

- Modify: `echo-sdk-protocol/src/capability.rs` — 将 ExtensionBridge 真实能力、callback limits 和可选 capability 的 advertisement/validation 固化。
- Modify: `echo-sdk-protocol/src/error.rs` — 补齐 extension rejected/failed/timeout/disconnected、conflict 和 late-response 的稳定错误数据边界。
- Modify: `echo-sdk-protocol/src/methods.rs` — 将注册、注销、反向调用、取消和每 kind operation 收敛为官方 typed RPC DTO，并加入 InterventionCallback kind。
- Modify: `echo-sdk-protocol/src/event.rs` — 固化 extension stream chunk/terminal、sequence、generation 和 cancellation notification 的 wire 规则。
- Modify: `echo-sdk-protocol/src/catalog.rs` — 保持五个 bridge 方法的 namespace、方向和 capability 一致，并补充 operation catalog。
- Modify: `echo-sdk-protocol/src/schema.rs` — 导出新的 bridge Schema、digest 和正反 fixture。
- Modify: `echo-sdk-protocol/tests/extension_contract.rs` — 验证 descriptor、operation、错误、stream 和边界的 round-trip 与 fail-closed 行为。
- Modify: `contracts/sdk/schema/echo-agent-extension-v1.schema.json` — 保存生成后的扩展合同。
- Modify: `contracts/sdk/fixtures/extension/v1/` — 增加每个 kind 的 register/invoke/stream/cancel/timeout/disconnect 正反样例。
- Modify: `contracts/sdk/source-contract.json` — 由固定 generator 更新 source compatibility digest。
- Modify: `contracts/sdk/parity-manifest.json` — 为 extension trait 与 bridge operation 保持唯一分类、ACP relationship 和当前语言状态。
- Modify: `src/acp/mod.rs` — 导出 connection-scoped extension authority 的公共边界。
- Modify: `src/acp/runtime.rs` — 为 profile/adapter 提供 extension invocation、cancel、close 和 settlement hook，不改变已有 Run authority。
- Modify: `src/acp/adapter.rs` — 在官方 close chain 中接入 extension admission/cancel/wait，并保持一次 builder composition。
- Create: `src/acp/extension.rs` — 实现 protocol-neutral registration/invocation leases、deadline、cancellation、concurrency gate 和 exactly-once settlement。
- Create: `tests/acp_extension_runtime.rs` — 覆盖共享 runtime 的并发、超时、取消、late response、重入冲突和关闭顺序。
- Modify: `echo-sdk-host/src/config.rs` — 增加 extension 数量、payload、stream、callback concurrency 和 timeout 的显式有界配置。
- Modify: `echo-sdk-host/src/core_profile/mod.rs` — 在协商后的 profile 中挂载 bridge builder 与 typed registration/stream handlers。
- Modify: `echo-sdk-host/src/core_profile/state.rs` — 持有 connection-owned bridge registry、invocation dispatcher 和 profile close 状态。
- Modify: `echo-sdk-host/src/core_profile/handles.rs` — 增加 Extension handle 的 kind/generation/closed/ABA/idempotency 校验与级联释放。
- Modify: `echo-sdk-host/src/core_profile/handler.rs` — 实现 register/unregister、反向调用结果、cancel/stream notification 的 admission、capability、事务回滚和错误映射。
- Create: `echo-sdk-host/src/core_profile/extension_bridge.rs` — 实现官方 ConnectionTo reverse dispatcher、每种 trait 的薄 proxy 和 SDK callback lifecycle adapter。
- Modify: `echo-sdk-host/src/core_profile/wire.rs` — 实现每 kind 的 Rust trait 输入/输出、ToolContext、Artifact、Store search、HITL response、Hook/Intervention result 的无损转换。
- Modify: `echo-sdk-host/src/core_profile/events.rs` — 复用现有有界事件/ACK/backpressure 规则传递 extension stream，不复制第二套队列。
- Modify: `echo-sdk-host/src/factory.rs` — 在每个独立 Session Agent 构造时注入当前 connection 的 extension proxies，并保持 AgentFactory 与 Session ownership 分离。
- Create: `echo-sdk-host/tests/extension_bridge_e2e.rs` — 使用官方 Client 与真实 source-built Host 验收所有 bridge kind、stream、cancel、timeout、disconnect、generation 和 close 场景。
- Modify: `echo-sdk-host/tests/core_profile_e2e.rs` — 验证 ExtensionBridge advertisement 不改变 core profile 与标准 Prompt/Run 共用 authority。
- Modify: `echo-sdk-protocol/tests/core_rpc_contract.rs` — 将 bridge capability、错误和 generation 互操作加入 core contract 回归。
- Modify: `docs/sdk/sdk-core-profile.md` — 说明 core profile 与 ExtensionBridge 的边界、协商和生命周期。
- Create: `docs/sdk/sdk-extension-bridge.md` — 记录注册、反向调用、trait 映射、取消、流和错误合同。
- Modify: `docs/sdk/README.md` — 导航 bridge 文档并保持 Runnable/Parity/Published 状态诚实。
- Modify: `docs/sdk/protocol.md` — 更新 extension typed RPC、operation、stream 和 error reference。
- Modify: `docs/sdk/acp-agent-adapter.md` — 说明标准 adapter、profile builder 与 reverse callback 的单一连接组合。
- Modify: `docs/adr/0028-source-first-multilanguage-sdk-runtime.md` — 记录双向 bridge 的 authority、无 fallback 和故障取舍。
- Modify: `README.md` — 记录源码构建 Host 已提供 ExtensionBridge，但语言客户端尚未 Runnable。
- Modify: `README.zh.md` — 同步中文状态、工具链前提和源码交付边界。
- Modify: `CHANGELOG.md` — 记录 bridge registration、reverse invocation、stream、cancel、timeout 和 disconnect 语义。
- Modify: `.github/workflows/rust-ci.yml` — 将 bridge contract/E2E 与现有 Host 分组门禁接入，不改变低资源分组策略。

## Reuse

- src/acp/runtime.rs:427-778 — AcpConnectionServices、RunEntry、AcpConnectionProfile — 复用唯一 Session/Run、admission、receipt、event observer 和 close chain。
- src/acp/adapter.rs:220-405 — 官方 ACP Builder composition — 保持 initialize、stdio、标准 handlers、profile handlers 和 reverse request 在同一连接。
- echo-sdk-protocol/src/methods.rs:885-1032 — ExtensionRegister/Unregister/Invoke/Cancel/Stream DTO 骨架 — 扩展为官方 typed RPC，而不是再建平行 envelope。
- echo-sdk-host/src/core_profile/handles.rs — HandleRegistry — 复用 generation、kind、closed、idempotency 和 max-open-handles 规则。
- echo-sdk-host/src/core_profile/state.rs — CoreProfileState — 复用 state root、advertisement、journal、delivery 和 settlement wait authority。
- echo-sdk-host/src/factory.rs:24-83 — PreparedAgentDefinition::create_agent — 保持每 Session 独立 Agent，并在构造时注入 proxy。
- src/agent/react/builder.rs:235-615 — ReactAgentBuilder 的 llm_client、tool、callback、store、state_store 注入点 — 复用既有 Agent 配置入口。
- src/agent/react/capabilities.rs:240-760 — ReactAgent 的 add/remove/replace tool、add_callback 和 hook registry 入口 — adapter 只做注册转换。
- echo-core/src/tools/mod.rs:876-1040 — Tool、ToolContext、ToolResult 和 streaming contract — 保留 artifact、failure、permission、context 和 stream 语义。
- echo-core/src/llm/mod.rs:397-451 — LlmClient chat/chat_stream/model_name/capabilities — 分别映射 non-stream 和 stream operation。
- echo-core/src/memory/store.rs:182-256 — Store put/get/search/search_with/delete/list/prune/dedup — 保留 namespace、search mode、limit 和原子结果。
- echo-orchestration/src/human_loop/mod.rs:805-808 — HumanLoopProvider::request — 复用 request identity 和一次响应结算。
- echo-core/src/agent/mod.rs:1604-1673 — AgentCallback — 保留 observational callback 顺序、kind/id 和可见输入。
- echo-core/src/agent/intervention.rs:64-126 — InterventionCallback/InterventionResult — 保留可拒绝、修改、注入、重定向和取消语义。
- echo-core/src/agent/factory.rs:41-148 — AgentFactory — 保持构造与 execute/chat stream 分离。
- /Users/ls/.cargo/registry/src/rsproxy.cn-e3de039b2554c837/agent-client-protocol-2.1.0/src/jsonrpc.rs:3310-3345 — ConnectionTo send_request/send_notification/send_cancel_request — 复用官方 reverse request、响应路由和 framing。

## Todos

### freeze-extension-bridge-contract

requirements:
- § 10.1 协议分层与通道纪律
- § 10.4 echo-agent SDK extension families
- § 10.6 错误合同
- § 12.1 统一模型
- § 12.2 Trait 映射
- § 18. 版本与兼容策略
- § 20.2 合同一致性

interfaces:
- consumes: Plan 05 已冻结的 ExtensionKind、WireValue、WireHandle、EchoSdkError、ExtensionStreamEvent 和 _echo_agent/extension/* catalog。
- produces: 每 kind 的 versioned descriptor/operation DTO、官方 JsonRpcRequest/Response/Notification 实现、ExtensionBridge capability limits、Schema/fixture/source-contract 更新。

steps:

1. 审核现有 ExtensionRegisterRequest、ExtensionInvokeCall、ExtensionInvokeOutcome、ExtensionCancelNotice 和 ExtensionStreamEvent 的 wire 形状，按 kind/operation 建立不含任意猜测的 versioned descriptor、context、result、stream、error 和 registration ownership 字段；将 InterventionCallback 显式纳入 ExtensionKind。
   verify: 每个字段都能指向设计的 trait mapping、identity、deadline、generation 或 bound，unknown field、空 identity、错误 kind 和超限 payload 被稳定拒绝。
   expected: 一个 canonical wire grammar 能区分 Tool、LlmClient、Store、HumanLoopProvider、Hook、AgentCallback、InterventionCallback、AgentFactory 和自定义 Agent，不依赖标准 ACP 私有字段。

2. 为 request/response/notification 补官方 typed derive 和 method catalog 绑定，固定 extension error code/data、stream terminal exactly-once、cancel notice 与 late-response 规则；更新 Schema、fixtures、source-contract 和 parity manifest。
   verify: 合同 generator 的默认检查无漂移，正反 fixture 分别通过/拒绝，标准 ACP 方法和官方 schema 没有被重新定义。
   expected: 语言 SDK 可以只消费生成合同实现 typed dispatcher，缺少 ExtensionBridge 或 digest 不匹配时保持 fail closed。

### build-shared-extension-runtime

requirements:
- § 12.1 统一模型
- § 12.3 并发与死锁约束
- § 14.2 反向扩展调用
- § 14.3 SDK 主动取消
- § 15. 异常与边界场景
- § 16. 安全与本地边界

interfaces:
- consumes: AcpConnectionServices、AcpConnectionProfile、RunEntry 的 admission/close/settlement hooks，以及官方 ConnectionTo 的 typed transport callback。
- produces: protocol-neutral ExtensionInvocationRegistry、ExtensionInvocationLease、ExtensionTransport boundary、cancel/close/wait hooks，供 Host bridge 和 ReactAgent proxies 使用。

steps:

1. 在 root ACP 模块增加 connection-scoped extension authority，注册 invocation key 为 extension handle generation 加 implementation/invocation identity，使用 bounded semaphore、per-call CancellationToken、deadline 和 single-settlement state。
   verify: 并发调用、相同 identity 重复响应、超时、取消、transport error 和 late response 都返回确定结果，任何路径都不阻塞 reader loop。
   expected: framework 只保存 invocation lifecycle，不保存语言对象、产品 UI 状态或第二套 Run terminal。

2. 将 extension runtime 接入 AcpConnectionProfile/Adapter 的 close chain：关闭 admission 后取消在途 callback，等待有界结算，再执行 profile flush 和 Session Agent close；标准 NoProfile 与 plain ACP 路径保持行为不变。
   verify: close、stdin EOF、自然完成和取消竞争下每个 invocation 最多一次 settlement，所有等待都有明确 timeout，标准 ACP tests 继续通过。
   expected: extension transport 断连不会把 framework Run 报告为 completed，也不会遗留可调用的 registry entry。

3. 增加 runtime regression tests，使用可控 fake transport 观察 callback reentry、concurrency permit、cancellation propagation、late response discard 和 release ordering。
   verify: 测试覆盖所有异常分支且无死锁、无无界队列、无 panic 或数据竞争。
   expected: Host bridge 可以替换 transport 实现而无需复制 lifecycle guard。

### integrate-host-extension-registry

requirements:
- § 10.2 初始化与双 profile 协商
- § 10.4 echo-agent SDK extension families
- § 13. Feature 模型
- § 16. 安全与本地边界
- § 17. 源码布局与交付合同

interfaces:
- consumes: CoreProfileState、HandleRegistry、PreparedAgentDefinition、AcpConnectionServices、ExtensionBridge typed DTO 和 shared extension runtime。
- produces: connection-owned extension registry、ExtensionBridge advertisement、register/unregister handlers、bridge-bound Session Agent construction context 和 typed host error mapping。

steps:

1. 扩展 SdkProfileLimits/EchoLimits，提供 max extensions、descriptor/payload/stream bytes、callback concurrency、callback timeout 和 registration lifetime 的显式有界配置；只在 feature 编译且 profile active 时声明 ExtensionBridge。
   verify: 配置缺失、零值、超限、未知字段和未启用 feature 在打开 stdio 前 fail closed；initialize advertisement 的 limits 与 Host 实际 enforcement 一致。
   expected: Client 能通过 capability snapshot 预知 bridge 是否可用和可接受的资源上限。

2. 在 CoreProfileState/HandleRegistry 增加 Extension registration records，复用既有 generation/kind/closed/idempotency 规则并绑定 connection ownership；register/unregister 与 agent/session close 做级联释放和并发复核。
   verify: 同 identity 同 descriptor 返回同 handle，不同 descriptor 返回 typed conflict；旧 generation、错误 kind、closed handle、重复 unregister 和 agent close 竞争均无状态泄漏。
   expected: extension handle 绝不跨 Host restart 或另一 ACP connection 静默复用。

3. 在 core profile attach/handler 中接入 register/unregister 与 extension stream/cancel inbound handlers，并把 reverse dispatcher 绑定到官方 ConnectionTo<Client>；所有 handler 使用 extended/capability/admission/shape/generation/limit 梯子。
   verify: plain Client 的 extension request 得到 method-not-found，协商但 capability 不存在得到 typed mismatch，注册或 dispatcher 失败会回滚所有已写入 registry/handle/index 状态。
   expected: 标准 handlers、core handlers 和 bridge handlers 仍由一次 official Builder composition 驱动。

4. 修改 PreparedAgentDefinition::create_agent 的构造上下文，使每个 Session Agent 获得当前 connection 的 bridge proxies；不把 ConnectionTo、锁或产品状态暴露给 framework trait 实现。
   verify: 多 Session 使用同一注册定义时 Agent 历史、cwd、MCP 和 bridge invocation identity 相互隔离；Agent close 能回收其 Session-scoped proxies。
   expected: AgentFactory 负责构造，SessionRegistry 负责 Session ownership，bridge 只负责反向调用。

### implement-trait-proxies-and-streams

requirements:
- § 11.1 事件权威
- § 11.2 Replay 与背压
- § 12.2 Trait 映射
- § 12.3 并发与死锁约束
- § 14.2 反向扩展调用
- § 14.3 SDK 主动取消
- § 15. 异常与边界场景

interfaces:
- consumes: Host extension registry/transport、echo-sdk-protocol per-kind operation DTO、Tool/LlmClient/Store/HumanLoopProvider/Hook/Callback/AgentFactory/Agent public traits。
- produces: 每种 Rust trait 的 thin proxy、typed input/output conversion、stream handle delivery 和 reverse-request settlement。

steps:

1. 实现 Tool proxy，完整传递 name/description/parameters/schema_revision/modality、ToolContext、permission/risk、ToolResult artifact/failure/metadata，并分别支持 execute、validate_parameters 和 real stream。
   verify: 工具成功、typed failure、artifact、context、timeout、cancel、disconnect 和 late result 均保留 Rust 的结果语义，不回退内置工具。
   expected: ToolManager 仍负责权限、retry、sandbox 和执行 policy，proxy 不复制这些策略。

2. 实现 LlmClient 与 stream proxy，区分 chat 和 chat_stream，传递 model/capabilities、request context、chunk identity、sequence、usage 和 terminal；通过 bounded stream event 处理 backpressure。
   verify: non-stream 与 stream 的 chunk、timeout、cancel、failed/complete terminal 和 consumer disconnect 均可观察且不会把大响应压成一次 WireValue。
   expected: AgentTurnDriver 继续决定 Run terminal，bridge 只返回 LLM trait 结果。

3. 实现 Store、HumanLoopProvider、Hook、AgentCallback 和 InterventionCallback proxy，保留 namespace/search mode/pagination、request identity/one-shot response、hook order/mutation/deny、observational callback 顺序和 intervention modify/inject/redirect/cancel。
   verify: 每类 proxy 都通过成功、typed rejection、timeout、cancel、disconnect、重复响应和 late response 场景；callback reentry 返回 typed conflict。
   expected: 标准 session/request_permission 仍是 ACP projection，不替代 HumanLoopProvider extension；Hook 与 Callback 不共享错误的结果类型。

4. 实现 AgentFactory 与自定义 Agent proxy，分离 construct、execute/chat stream、close 和 cancellation；将事件交回现有 EventEnvelope/Run observer，不由 proxy 伪造 sequence、terminal 或 receipt。
   verify: 自定义 Agent 创建/执行/stream/close 与失败路径都经过 generation/invocation fence，Session Agent 关闭后没有可调用 proxy。
   expected: framework 的 AgentFactory、Agent、Run、Session 和 state store authority 保持唯一。

5. 增加真实 Host/官方 Client E2E，覆盖每个 kind 的 register/invoke/unregister、stream terminal、cancel notice、deadline、disconnect、host close、generation stale/closed、descriptor/payload bound 和 secret/stdout isolation。
   verify: 真实 source-built Host 在 sdk-core-profile + ExtensionBridge 下所有正向场景成功，所有负向场景返回指定 typed error，标准 ACP regression 不变。
   expected: bridge E2E 可作为后续 TS/Python/Java SDK 的同一行为基线。

### prove-bridge-reliability-and-docs

requirements:
- § 18. 版本与兼容策略
- § 19. 文档与示例
- § 20.3 行为一致性
- § 20.4 可靠性
- § 20.5 源码交付
- § 20.6 状态声明

interfaces:
- consumes: bridge contract、runtime/Host E2E evidence、root facade parity manifest 和现有 docs/sdk 唯一入口。
- produces: 可复核的 bridge reliability evidence、更新后的公开文档/ADR/CHANGELOG/CI gate，并保持语言 SDK 状态为未 Runnable。

steps:

1. 将 bridge contract tests、runtime tests 和真实 Host E2E 纳入现有 Rust/Host 验证链，执行仓库规定的 fmt、clippy、workspace test、no-default 和适用 feature matrix；合同重新生成后确认无漂移。
   verify: ./scripts/verify.sh 与适用 feature matrix 全部退出 0，bridge fixture/schema/source-contract/public-api/parity 文件没有未提交生成差异。
   expected: bridge 的协议、生命周期、故障、资源和 feature 边界都有命令或真实进程证据。

2. 更新 docs/sdk 唯一入口、protocol reference、ACP adapter/core profile 文档、ADR、README、中文 README 和 CHANGELOG，说明 registration scope、reverse invocation、stream/cancel/error、源码构建前提和不提供预编译产物的边界。
   verify: 文档中的 method、capability、limits、trait mapping、错误和状态与合同/测试逐项一致，所有本地链接可解析。
   expected: 外部读者能区分 Design、Contract、ACP conformant、Core extension profile、Runnable、Parity complete 和 Published。

3. 复核 .github/workflows/rust-ci.yml 的分组门禁，保持 Host E2E 的资源隔离和 stderr/stdout 诊断边界；不把 CI 低资源限制转化为放宽 bridge 行为契约。
   verify: CI 配置只增加 bridge 的独立验证入口，不删除原有分组、Windows 编译、依赖审计或标准 ACP 检查。
   expected: 本地完整门禁和 CI 环境差异各自承担独立信号，任何失败都能定位到合同、runtime、Host 或 fixture。

## Decisions

- ExtensionBridge 作为 sdk-extension-bridge outcome 独立交付；它完成后即具有可复用价值，但不提前承诺 facade-feature-adapters 或任何语言 SDK 已完成。
- InterventionCallback 显式纳入本计划，因为它是根 facade 的公开行为控制 trait；若某个 Host feature 未编译，只能返回 feature_unavailable，不能映射成 observational callback 或静默忽略。
- 注册记录按 ACP connection generation 所有，不写入跨重启 callback 句柄；Host 重启后语言 SDK 必须重新注册，历史 Run/Session 恢复不复活任意在途 callback。
- 采用 per-kind typed descriptor/operation 加统一 invocation envelope，而不是让每种语言直接提交任意 WireValue；这样能保持 Rust trait 字段、错误和流式语义可验证，又不复制官方 ACP 基础 envelope。
- 反向调用通过官方 ConnectionTo 与同一 Builder composition 完成，reader loop 只分派消息；不实现第二套 JSON-RPC client、线程池或 transport parser。
- Extension failure 不触发隐式 built-in fallback；Rust API 若未来需要显式 fallback，必须作为新的合同字段和独立兼容决策出现。
- 当前计划结束时状态最多推进到 Contract、ACP conformant 与 Core extension profile available；TypeScript、Python、Java 的 Runnable、Parity complete、Published 状态仍由后续 outcome 决定。