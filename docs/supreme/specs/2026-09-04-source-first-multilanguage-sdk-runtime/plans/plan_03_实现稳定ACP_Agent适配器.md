---
schema_version: 3
supersedes: null
slug: 2026-09-04-source-first-multilanguage-sdk-runtime/plan
goal: 让根 echo_agent 通过可选 acp feature 提供可组合的稳定 ACP v1 Agent adapter，把标准 Session 与
  Prompt Turn 无损接入现有框架执行、事件和取消权威。
ships: 根 echo_agent 的可选 acp feature、复用官方 Rust SDK 的通用 ACP Agent adapter，以及对稳定 v1
  initialize/session/prompt/update/cancel 与协商能力的 conformance 证据。
verify: 官方 ACP Client 经真实 typed connection 完成 initialize、session/new、连续
  prompt、session/update 与终态，并能通过 session/cancel 和 request cancellation 取消在途
  Prompt；每个 Session 的 Agent 与历史隔离，未实现 capability 不被宣告，./scripts/verify.sh 与包含
  acp 的独立 feature 条件矩阵全部通过且零警告。
design_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md
delivery_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/plans/delivery-map.md#acp-agent-adapter
todos:
  - id: establish-acp-adapter-boundary
    files:
      - Cargo.toml
      - Cargo.lock
      - AGENTS.md
      - src/lib.rs
      - src/acp/mod.rs
      - src/acp/adapter.rs
      - src/acp/session.rs
      - echo-agent-learning/Cargo.toml
    summary: 接入精确锁定的官方稳定 ACP Rust runtime，建立 transport-neutral adapter、逐 Session
      Agent 构造边界和最小状态所有权。
    verify: acp 可独立启用且不启用任何 unstable ACP feature；adapter 可作为官方 Agent role 连接任意
      transport，每个 session/new 获得独立 Agent，registry 只拥有 Session 资源和当前取消令牌。
  - id: drive-standard-acp-turns
    files:
      - src/acp/adapter.rs
      - src/acp/session.rs
      - src/acp/prompt.rs
      - src/acp/projection.rs
    summary: 把 initialize、session/new、session/prompt、session/update、session/cancel 和
      request cancellation 接入 AgentTurnDriver 与 EventEnvelope。
    verify: Prompt handler 不阻塞官方 dispatch loop；文本和 ResourceLink
      输入被确定性保留，事件按序投影，Completed 与 Cancelled 产生正确 StopReason，framework failure 作为
      ACP error 结算且不关闭连接。
  - id: prove-acp-v1-behavior
    files:
      - tests/acp_agent_adapter.rs
      - contracts/sdk/fixtures/acp/v1
    summary: 使用官方 Client 与 typed messages 建立稳定 ACP v1 行为、并发、取消和失败 conformance 证据。
    verify: 真实连接覆盖初始化协商、唯一 Session ID、两会话隔离、连续多轮、update 顺序、未知 Session、并发 Prompt
      拒绝、两种取消竞争、连接关闭清理和无死锁。
  - id: sync-acp-public-surface
    files:
      - README.md
      - README.zh.md
      - CHANGELOG.md
      - docs/adr/0028-source-first-multilanguage-sdk-runtime.md
      - docs/sdk/README.md
      - docs/sdk/protocol.md
      - docs/sdk/acp-agent-adapter.md
      - echo-agent-learning/Cargo.toml
      - echo-agent-learning/examples/demo72_acp_agent_adapter.rs
      - echo-agent-learning/examples/README.md
      - echo-sdk-protocol/src/inventory.rs
      - contracts/sdk/public-api.txt
      - contracts/sdk/parity-manifest.json
    summary: 同步正式文档、可编译示例、ADR 实施事实与新增 acp facade 的 parity inventory。
    verify: 文档只声明已实现的 adapter 能力并保持 Host/三语言 SDK 未完成状态；示例进入 all-targets 编译；public
      inventory 与 parity manifest 对新增 acp API 零缺项且 extension schema 不复制标准 ACP
      类型。
artifact_id: plan:e007a4ae-73ef-4c54-9fc2-87ca413bbb7f
lifecycle: completed
design_revision: sha256:9685cabd8f692ea060b67e46fe1c7d47531edb44efd998913e252803ef122817
---
## Approach

- 使用官方 `agent_client_protocol::Agent.builder()`、typed handler 与 `ConnectTo<Client>` 作为唯一 JSON-RPC/ACP runtime；adapter 本身不绑定 stdio，下一交付的 SDK Host 只负责提供默认 Agent 配置并连接 transport。
- 每次 `session/new` 通过 `AcpSessionFactory` 接收完整 cwd、additional directories、MCP declarations 与 initialize capability snapshot，返回一个新的框架 `Agent`。该协议适配 hook 可委托现有 `AgentFactory`，不拥有对话历史或执行语义。
- Session registry 只保存独立 Agent、Session 配置和当前 turn identity/token。Prompt 使用 `EventIdentity::for_chat`、`AgentInvocationContext`、`TurnRequest::Chat` 与 `AgentTurnDriver::drive`；历史、终态、计账和事件 sequence 继续由框架权威产生。
- `session/prompt` handler 先登记 active token，再用官方 connection task 执行 Driver，使 dispatch loop 可继续处理 `session/cancel` 与 `$/cancel_request`；结算时按 turn identity 条件清除，迟到取消不能命中下一 Turn。
- `EventSink` 只把同一个已接受 `EventEnvelope` 投影为标准 `session/update`。无法由 ACP 无损表达的字段保留在框架事实中，等待后续 `_echo_agent/*` event profile；adapter 不生成第二套 sequence 或终态。
- 初始化只宣告本阶段真实可用的稳定 v1 capability。`_meta.echo_agent` 在对应 extension handlers 可达前不发布；draft v2 与全部 `unstable_*` feature 保持关闭。

## Global Constraints

- 该能力属于通用 `echo-agent` 框架，不依赖 `echo-agent-cli`、EKO workspace、GUI/TUI 投影或产品配置。
- `agent-client-protocol` 固定 `=2.1.0`，标准 schema 继续由官方 crate 提供；不得复制 ACP envelope、Session、Prompt、update、request ID 或 stop reason。
- 新根 feature 名为 `acp`，同时进入 root `full`、docs.rs 和 learning crate feature 镜像；默认与 no-default 构建不暴露 ACP module，独立 feature 矩阵必须包含 `acp`。
- 本计划不创建 `echo-agent-sdk-host` binary，不实现 `_echo_agent/*` request handler、语言 SDK、通用 ACP Client、Proxy、Conductor、draft v2 或 unstable extension。
- 每个 ACP Session 必须拥有独立 Agent 实例；同一 Session 同时只允许一个 active Prompt，两个 Session 可以并发。
- ACP baseline 输入至少支持 Text 与 ResourceLink。Image、Audio、embedded Resource 只有完成确定性映射并真实宣告 capability 后才可接受。
- 所有取消源汇合到同一个 framework `CancellationToken`；只有 `TurnReceipt` 决定终态，EOF、连接错误或 stream 返回本身不得视为成功。
- 不使用 `unwrap`、`expect`、panic API、可能越界的直接索引或 UTF-8 字节截断；协议 payload、diagnostic 与投影保持有界。
- 只交付源码和可编译示例，不发布或下载项目构建的 binary、npm package、wheel、JAR 或语言 runtime。

## Files

- Modify: `Cargo.toml` — 接入可选官方 ACP runtime/contract 依赖，声明 `acp` feature 并纳入 `full` 与 docs.rs。
- Modify: `Cargo.lock` — 记录根 crate 对已锁定 ACP runtime 与 contract crate 的依赖边。
- Modify: `AGENTS.md` — 将新独立 `acp` feature 纳入条件矩阵。
- Modify: `src/lib.rs` — 在 `acp` feature 下公开框架 ACP facade。
- Create: `src/acp/mod.rs` — 定义公共 adapter API 与模块边界。
- Create: `src/acp/adapter.rs` — 组合官方 Agent builder、typed handlers 与 transport-neutral connection。
- Create: `src/acp/session.rs` — 保存单连接 Session 资源、独立 Agent 与 active turn cancellation slot。
- Create: `src/acp/prompt.rs` — 将稳定 v1 ContentBlock 确定性映射到 framework Message/TurnRequest。
- Create: `src/acp/projection.rs` — 将 EventEnvelope 与 TurnReceipt 投影为标准 update、stop reason 和 ACP error。
- Create: `tests/acp_agent_adapter.rs` — 通过官方 Client/Channel 验证真实协议主路径与取消并发。
- Create: `contracts/sdk/fixtures/acp/v1` — 保存项目侧稳定 v1 traffic 正反例，不复制官方 schema。
- Modify: `echo-agent-learning/Cargo.toml` — 镜像 root `acp` feature 并注册编译示例。
- Create: `echo-agent-learning/examples/demo72_acp_agent_adapter.rs` — 展示自定义 Session factory 与 transport 组合。
- Modify: `echo-agent-learning/examples/README.md` — 收录 ACP adapter 示例和适用边界。
- Modify: `README.md` — 增加 `acp` feature 与当前 adapter 能力。
- Modify: `README.zh.md` — 同步中文 feature 与能力说明。
- Modify: `CHANGELOG.md` — 记录稳定 ACP Agent adapter 增量。
- Modify: `docs/adr/0028-source-first-multilanguage-sdk-runtime.md` — 记录 adapter 的实际 Session、Driver 与取消边界。
- Modify: `docs/sdk/README.md` — 将状态推进到 adapter 可用并保持 Host/SDK 尚未交付。
- Modify: `docs/sdk/protocol.md` — 说明稳定 profile 的 handler、projection、capability 与失败行为。
- Create: `docs/sdk/acp-agent-adapter.md` — 提供公共 Rust adapter 的构造、Session factory 与 transport 使用说明。
- Modify: `echo-sdk-protocol/src/inventory.rs` — 为新增 root ACP facade 固化准确 relationship 与语言映射规则。
- Modify: `contracts/sdk/public-api.txt` — 重新生成包含 `acp` feature 的根 facade inventory。
- Modify: `contracts/sdk/parity-manifest.json` — 重新生成并审阅新增 ACP facade 的唯一对等映射。

## Reuse

- `echo-orchestration/src/runtime/turn_driver.rs:103` — `TurnRequest` — 复用 Chat、identity、invocation、取消和续接 sequence 输入合同。
- `echo-orchestration/src/runtime/turn_driver.rs:418` — `TurnReceipt` / `TurnOutcome` — 复用唯一终态、最终答案和计账事实。
- `echo-orchestration/src/runtime/turn_driver.rs:517` — `EventSink` — 作为标准 ACP update 的薄投影边界。
- `echo-orchestration/src/runtime/turn_driver.rs:544` — `AgentTurnDriver::drive` — 保持 Agent 执行和 exactly-one-terminal 权威。
- `echo-core/src/agent/event_envelope.rs:230` — `EventIdentity::for_chat` — 将 ACP Session/Prompt identity 接到既有事件链。
- `echo-core/src/agent/mod.rs:416` — `AgentInvocationContext` — 传递 cwd、conversation 与 cancellation 上下文。
- `echo-core/src/agent/factory.rs:144` — `AgentFactory` — 普通 Agent 构造仍由现有框架工厂完成，ACP Session hook 只补充协议上下文与异步准备。
- `src/headless.rs:187` — 现有生产调用示例 — 复用构造 `TurnRequest` 后交给 Driver 的模式。
- `echo-sdk-protocol/Cargo.toml:19` — 已锁定官方 ACP artifacts — 根 feature 复用同一精确版本，不产生第二份依赖基线。
- `echo-sdk-protocol/src/inventory.rs` — facade inventory/parity 生成器 — 新 ACP API 进入同一公共面权威。

## Todos

### establish-acp-adapter-boundary

requirements:
- § 3.3 ACP、MCP 与 A2A
- § 4.1 已有通用权威
- § 4.2 已有能力但不足以直接作为 SDK Host
- § 6.1 框架层
- § 13. Feature 模型
- § 17. 源码布局与交付合同

interfaces:
- consumes: 官方 `agent_client_protocol::Agent.builder()` / `ConnectTo<Client>`、现有 `AgentFactory` / `Agent`、已锁定 `echo_sdk_protocol` 合同。
- produces: `echo_agent::acp::{AcpAgentAdapter, AcpAdapterConfig, AcpSessionFactory, AcpSessionContext}` 与 root `acp` feature。

steps:

1. 在 root manifest 中以 optional exact dependency 接入官方 ACP runtime 与本地 contract crate，新增 `acp` feature，并同步 `full`、docs.rs、learning mirror 和独立 feature 条件矩阵。
   verify: root feature topology 与 dependency feature tree。
   expected: `acp` 可独立编译，默认/no-default surface 不出现 ACP module，依赖树不启用任何 `unstable_*` ACP feature。
2. 建立公开、transport-neutral 的 adapter 配置与 Session 构造接口；Session context 完整携带协议 Session ID、cwd、additional directories、MCP declarations 和 initialize client capability snapshot。
   verify: 构造边界的字段级测试与两个 session/new 的 factory 观测。
   expected: 每次 Session 创建拿到完整独立上下文并产生新 Agent；adapter 不读取 EKO 配置，也不保存 transcript 或自建 Run 状态。
3. 建立 connection-scoped Session registry 与 active turn slot，定义唯一 Session ID、并发 Prompt 拒绝、turn identity compare-and-clear 和 connection close 清理。
   verify: Session/turn registry 的并发与生命周期行为。
   expected: 同一 Session 最多一个 active turn，迟到取消/清理不能影响新 turn，连接关闭会取消全部在途 turn 并关闭 Agent。

### drive-standard-acp-turns

requirements:
- § 10.1 协议分层与通道纪律
- § 10.2 初始化与双 profile 协商
- § 10.3 标准 ACP operation families
- § 10.5 路径、数值与投影边界
- § 10.6 错误合同
- § 11.1 事件权威
- § 14.1 正常执行
- § 14.3 SDK 主动取消
- § 15. 异常与边界场景

interfaces:
- consumes: `AcpAgentAdapter`、Session registry、官方 v1 typed messages、`AgentTurnDriver`、`EventEnvelope<AgentEvent>`。
- produces: initialize/session-new/session-prompt/session-update/session-cancel handlers、`AcpPrompt` mapper、`AcpEventProjector` 与 framework-to-ACP terminal/error mapping。

steps:

1. 用官方 typed handlers 实现 initialize 与 session/new；版本协商遵守官方规则，只宣告当前真实支持的 standard capabilities，并在 extension handler 可达前不发布 `_meta.echo_agent`。
   verify: standard-only Client 的 initialize 与 session/new 交互。
   expected: 版本与 capability 正确，未知/未实现方法由官方 runtime 处理，Session ID 唯一且上下文无损进入 factory。
2. 将 Text 与 ResourceLink prompt 内容转为 structured framework Message；未宣告的内容类型明确失败，绝不静默丢字段或损坏路径。
   verify: 多块文本、Unicode、ResourceLink 与未支持内容的正反例。
   expected: 支持输入保持顺序和关键 metadata，中文/emoji 不 panic，未支持 payload 在执行前返回稳定 ACP error。
3. Prompt handler 登记 shared cancellation 后立即把 Driver 工作交给官方 connection task；用 Chat mode 和 invocation context 驱动独立 Agent。
   verify: Prompt 运行期间 dispatch loop 仍可处理 session/cancel、request cancellation 和第二个 Session。
   expected: cancel 不死锁；同 Session 并发 Prompt 明确拒绝；不同 Session 可并发；单 Prompt 失败通过 responder 结算且 connection 继续可用。
4. 实现 stateful EventSink projector：Token/Think boundary、ToolCall/ToolStream/ToolResult 与最终答案投影成标准 update，receipt 映射 EndTurn/Cancelled 或 ACP error。
   verify: 同一 framework event 流的 update 顺序、tool call identity、无重复 final text 与 exactly-one response。
   expected: ACP update 只来自已接受 envelope；不生成新 framework sequence/状态，取消始终返回 `StopReason::Cancelled`，未知 framework failure 不被猜成其它 stop reason。

### prove-acp-v1-behavior

requirements:
- § 20.2 合同一致性
- § 20.3 行为一致性
- § 20.4 可靠性

interfaces:
- consumes: 完整 adapter、官方 `Client.builder()` / `Channel`、typed ACP v1 requests/notifications、framework mock Agent。
- produces: 可重复的 stable v1 protocol/concurrency conformance suite 与项目侧 traffic fixtures。

steps:

1. 用官方 Client role 与 in-process Channel 运行真实 initialize → session/new → prompt → updates → response 流程，不使用自建 JSON-RPC parser 或替代 DTO。
   verify: typed Client 观察到 capability、Session、按序 update 和正确 StopReason。
   expected: 所有 handler 经官方 dispatch 真正可达，两个 Session 的 Agent/history 不互串，连续两轮保留同一 Session 上下文。
2. 覆盖 unknown Session、idle cancel、同 Session 并发 Prompt、framework failure、connection close、session/cancel 与 `$/cancel_request` 的自然完成竞争。
   verify: bounded timeout 下的 terminal、error、cleanup 与 connection 可继续使用。
   expected: 无死锁、无 panic、无双重响应、无迟到取消污染；取消竞争最终只有一个 framework 权威结果。
3. 保存最小 traffic fixtures 并验证 method、标准字段与 StopReason 只来自官方 schema；fixture 不复制整份官方协议。
   verify: fixtures 可由官方 typed messages 编解码且非法顺序/字段被稳定拒绝。
   expected: 项目证据覆盖 adapter 行为，同时官方 schema 仍是唯一 wire 真理源。

### sync-acp-public-surface

requirements:
- § 7.1 权威集合
- § 7.2 Parity manifest
- § 18. 版本与兼容策略
- § 19. 文档与示例
- § 20.1 公共面完整性
- § 20.6 状态声明

interfaces:
- consumes: 新 `echo_agent::acp` public surface、adapter conformance evidence、现有 inventory generator 与 SDK 状态模型。
- produces: 编译示例、正式 adapter 文档、ADR 实施记录、更新后的 public-api inventory 与 parity manifest。

steps:

1. 增加通过 public facade 构造 Session factory 与 adapter 的 learning 示例，并把它纳入 `acp` required-feature 和 workspace all-targets 编译链。
   verify: 示例只依赖公开 API，并在无预编译 Host 或语言 runtime 时可从源码编译。
   expected: 开发者能看清 adapter 与 Session factory 的组合方式；示例不冒充后续标准 Host binary。
2. 更新 README、SDK 文档、CHANGELOG 和 ADR，明确稳定方法、capability、Session 隔离、事件投影、取消与当前完成度。
   verify: 文档声明与 conformance 证据逐项一致。
   expected: 状态仅推进为 ACP Agent adapter available；`ACP conformant`、Host runnable、语言 SDK runnable/published/parity complete 仍保持未完成。
3. 扩展 inventory 分类规则并重新生成 public-api/parity artifacts，审阅新增 ACP facade 的 feature 与 relationship。
   verify: SDK contract drift check 与字段级 manifest 检查。
   expected: 每个新增 `echo_agent::acp` public item 只出现一次、feature 为 `acp`、ACP relationship 准确，三语言状态仍诚实记录为未实现。
4. 执行仓库全量门禁和包含 `acp` 的条件 feature 矩阵，单独检查官方 ACP dependency feature tree。
   verify: `./scripts/verify.sh`、AGENTS.md 条件矩阵与依赖 feature 检查。
   expected: 所有适用命令退出 0，零 warning、零 fmt diff、零 contract drift、零 unstable ACP feature。

## Decisions

- 分层选择：adapter、official schema/runtime 组合、standard projection 与 conformance 属于任何 echo-agent 复用方都需要的框架机制；EKO 的 workspace、UI、外部 Agent 选择与产品持久化不进入本计划。
- 重复性搜索：仓库只有 A2A server、单次 headless 调用和已冻结 ACP contract，没有 root `acp` feature、ACP handler 或 Session registry。A2A 自己维护 task 状态且直接读 stream，不能作为 ACP 实现复用；ACP 复用 `AgentTurnDriver`。
- Session 构造选择 adapter-specific `AcpSessionFactory`，因为 ACP setup 包含异步 MCP 准备、cwd 与 client capability snapshot，现有同步 `AgentFactoryConfig` 无法无损表达。该 hook 只交付一个新 framework Agent，可在实现中委托 `AgentFactory`，不拥有执行、历史或终态。
- 采用官方 Rust SDK 的 typed builder/handler/connection 模式，依据 ACP 官方 Initialization、Session Setup、Prompt Turn、Cancellation 与 Extensibility 文档；这与设计中 Codex SDK、Claude Agent SDK 的单一引擎加结构化进程边界一致。
- Prompt 必须从串行 dispatch handler 派生为 connection task；否则 handler await 完整 Agent turn 时，`session/cancel` 无法进入，形成协议级死锁。
- 本交付只承诺稳定 v1 baseline methods 和真实 standard capability；扩展 method handler 未交付前不发布 echo-agent extension capability，避免协商成功后调用必然 method-not-found。
- `echo-website` 本阶段不修改，因为 delivery map 明确要求在 ACP conformance 与三语言 parity closeout 后才公开 SDK 入口；`echo-agent-cli` 不修改，因为 adapter 是通用框架能力。