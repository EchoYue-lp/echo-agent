---
title: echo-agent 基于 ACP 的源码优先多语言 SDK Host 设计
artifact: design
carrier: markdown
---

# echo-agent 基于 ACP 的源码优先多语言 SDK Host 设计

## 1. 问题与目标

`echo-agent` 当前以 Rust crate 形式提供完整框架能力。TypeScript、Python 和
Java 开发者若要使用这些能力，既不能直接消费 Rust 的 trait、泛型、生命周期和异步
stream，也不应在三种语言中各自重写 ReAct、TaskRun、Subagent、Memory、MCP、取消、
恢复与事件状态机。

本设计的目标是让三种语言对根 crate `echo_agent` 的正式公共 facade API 达到**功能和
语义全部对等**：同一能力拥有相同的状态转换、事件、错误、取消、恢复、并发和资源约束，
同时采用各语言惯用的 API 表达。

交付采用源码优先模式。仓库提供 Rust SDK Host、ACP adapter、echo-agent SDK 扩展合同
和三种语言 SDK 的源码；项目不发布预编译 Host 二进制、npm package、Python wheel 或
Java JAR。开发者从同一个 Git revision 自行编译全部产物。

SDK Host 同时提供两个兼容 profile：标准 ACP v1 profile 允许任何 ACP Client 调用
echo-agent；echo-agent SDK profile 在 ACP v1 上协商 `_echo_agent/*` 扩展，以无损覆盖
根 facade 的全部公共能力。两个 profile 投影同一个框架执行与状态权威。

## 2. 已确认决策

1. 多语言 SDK 的对象是通用框架 `echo-agent`，不是 EKO 应用。
2. Rust 实现是唯一执行权威；TS、Python、Java 不重写 Agent 框架。
3. 三语言通过一个独立 Rust 进程使用框架能力。
4. 该进程统一称为 **SDK Host**，可执行文件名为 `echo-agent-sdk-host`。这里的 Host
   不是 Node.js、Python 或 JVM 语言运行时。
5. Host 实现稳定 ACP v1 Agent role，使用 ACP 规定的 JSON-RPC 2.0 双向 stdin/stdout
   通信；不自建另一套基础 envelope、session 或 prompt 协议。
6. 项目只交付源码。开发者负责准备 Rust 和相应语言工具链并自行编译。
7. 对等是功能与语义对等，不是 Rust 符号、类型签名或内存布局同构。
8. 权威公共面是根 crate `echo_agent` 对外公开并正式文档化的 API，包含所有公开
   feature；workspace 子 crate 的内部 `pub` 项不是 SDK 承诺。
9. 各语言 API 必须保持惯用表达，不为追求表面一致而复制 Rust 所有权语法。
10. 完整 SDK 能力使用 ACP 官方扩展机制：初始化 `_meta` capability 加
    `_echo_agent/*` request/notification；扩展不得修改标准 ACP 字段或方法语义。
11. 首期只要求 SDK Host 实现 ACP Agent role，TypeScript、Python、Java SDK 实现 ACP
    Client role。通用 ACP Client、Proxy 和 Conductor 支持是后续独立能力。
12. ACP 标准视图允许按标准 schema 做有界投影；完整 SDK profile 必须通过扩展保留所有
    framework identity、状态、错误、恢复和 extension 语义。

## 3. 业界参考与取舍

### 3.1 OpenAI Codex SDK

[Codex SDK](https://developers.openai.com/codex/sdk/) 的 TypeScript SDK 包装本地 Codex
CLI，通过 JSONL 事件与子进程通信。它证明了“一个权威引擎 + 语言 SDK + 结构化进程
协议”适合长时间、流式、工具驱动的编码 Agent。

本项目采用相同的权威边界，但有两点不同：`echo-agent` 同时支持 TS、Python、Java，
并且只交付源码，不随 SDK 分发预编译引擎。

### 3.2 Claude Agent SDK

[Claude Agent SDK](https://code.claude.com/docs/en/agent-sdk/overview) 为 TypeScript 和
Python 提供语言惯用接口；对未提供原生 SDK 的语言，官方建议以结构化输出驱动 CLI
子进程。其区分“语言 API”与“Agent loop 权威”的方式支持本设计的单 Rust 内核方向。

### 3.3 ACP、MCP 与 A2A

[ACP Overview](https://agentclientprotocol.com/protocol/overview) 将编辑器/交互客户端与编码
Agent 的通信标准化为双向 JSON-RPC，覆盖初始化、Session、Prompt、流式 update、取消、
权限、文件、终端、计划和模式。Agent 通常作为 Client 启动的子进程，这与本项目选定的
SDK Host 进程边界一致。

ACP 官方提供 [Rust SDK](https://github.com/agentclientprotocol/rust-sdk) 以及 TypeScript、
Python、Java SDK。稳定 wire protocol 与 crate/schema artifact 版本独立，并通过
`initialize.protocolVersion` 与 capability 协商兼容性。项目直接复用官方 SDK，不复制
ACP schema、JSON-RPC envelope、Agent role runtime 或 conformance fixtures。

[ACP Extensibility](https://agentclientprotocol.com/protocol/v1/extensibility) 规定自定义数据
进入 `_meta`，自定义方法必须以下划线开头并在 capability 中声明。这使完整 echo-agent
SDK 可以建立 `_echo_agent/*` 扩展，同时保持标准 ACP Client 的互操作性。

[MCP SDK](https://modelcontextprotocol.io/docs/2026-07-28/sdk) 在多语言中保持相同协议
能力，同时允许各语言采用自身惯例；[A2A](https://github.com/a2aproject/A2A) 使用
JSON-RPC、流式事件和多语言 SDK 支持 Agent 互操作。

本项目复用这一“协议语义一致、语言表面不同”的原则。ACP 连接交互 Client 与编码
Agent，MCP 连接 Agent 与工具/资源，A2A 连接 Agent 与 Agent；三者职责不同，不互相
替代。SDK扩展不得重新定义 MCP 工具服务或 A2A Agent 互操作协议。

### 3.4 未选择 FFI 与多语言重写

[UniFFI](https://mozilla.github.io/uniffi-rs/latest/) 不能用一套成熟绑定同时覆盖
TypeScript、Python 和 Java；N-API、PyO3、JNI 仍需三套异步、回调、错误和二进制矩阵。
Rust 官方也要求显式处理跨 FFI unwind 边界，边界错误可能终止宿主进程，见
[Rust Nomicon](https://doc.rust-lang.org/nomicon/ffi.html#ffi-and-unwinding)。

逐语言重写虽然最原生，但会制造四套 Agent 语义权威。MCP、A2A 能维护多语言独立
实现，是因为它们首先维护正式规范、兼容测试和独立语言团队；`echo-agent` 当前是 Rust
框架，不应将 SDK 工作演变为三个新框架。

## 4. 当前代码事实与复用结论

### 4.1 已有通用权威

- `echo-core/src/agent/event_envelope.rs` 已提供带 schema version、稳定 identity、
  单调 sequence、parent link 与 content hash 的 `EventEnvelope`。
- `echo-orchestration/src/runtime/turn_driver.rs` 已提供 `AgentTurnDriver`，统一 Chat 与
  Execute 的事件驱动、唯一终态、取消和 `TurnReceipt`。
- `echo-orchestration/src/tasks/runtime_service.rs` 已提供 revisioned Task graph、claim、
  retry、pause/resume、interruption safe point 等通用任务机制。
- 根 facade 已公开 `Agent`、`ReactAgentBuilder`、`Tool`、`LlmClient`、`Store`、
  `HumanLoopProvider`、Subagent、MCP、A2A、workflow、memory、tracing 等能力。
- `serde` 已覆盖主要线值；`schemars` 已用于 JSON Schema；Tokio 已覆盖异步进程与 I/O。
- 官方 `agent-client-protocol` Rust SDK 已提供 ACP schema、Agent/Client runtime、stdio
  connection 与 conformance 基础，应作为 ACP wire 事实源。

SDK Host 必须适配这些权威，不得建立另一套 Run、Task、Subagent、事件、重试、取消或
恢复状态机。

### 4.2 已有能力但不足以直接作为 SDK Host

- `src/headless.rs` 是单次、非交互调用，不提供长期 Host、多 Agent、多 Session、双向
  callback 或统一 extension 生命周期。
- `src/a2a/` 面向 Agent 互操作，只提供 A2A 方法及其任务模型，不等于框架完整公共 API。
- 当前仓库没有 `acp` feature、ACP Agent adapter、ACP session/update 投影或ACP
  conformance tests。
- EKO 的 `echo-agent-cli --jsonl` 与 Tauri `ts-rs` 类型属于应用合同，不是通用框架
  SDK 合同，不能反向成为 `echo-agent` 的公共权威。

因此复用执行、事件、任务与既有协议实现，引入官方 ACP SDK作为标准通信层，并只为
ACP没有表达的 facade 能力新增最小 `_echo_agent/*` 扩展合同。

## 5. 范围与非目标

### 5.1 范围

- 根 `echo_agent` facade 的全部正式公共能力及所有公开 feature。
- TypeScript、Python、Java 的语言惯用 SDK。
- Rust SDK Host 源码及双向本地进程协议。
- `echo-agent` 的稳定 ACP v1 Agent adapter 与 `acp` feature。
- 标准 ACP Client 可用的 initialize、session、prompt、update、permission、filesystem、
  terminal、plan、mode 和 cancellation 映射。
- 基于 ACP `_meta` 与 `_echo_agent/*` 方法的完整 SDK扩展 profile。
- 值类型、命令、事件、stream、handle、callback extension 的跨语言映射。
- Agent、Session、Turn、Run、TaskRun、Subagent 的创建、查询、控制和观察。
- Tool、LlmClient、Store、Hook/Callback、HumanLoopProvider、AgentFactory 等外部实现点。
- ACP、MCP、A2A、memory、workflow、tracing、eval、improve 和 feature-gated 能力。
- 协议版本、feature 协商、错误闭集、取消、恢复、背压和进程退出语义。
- 源码构建、示例、合同测试、跨语言一致性测试和公共文档。

### 5.2 非目标

- 不为 EKO 的 GUI、TUI、workspace policy 或产品投影提供 SDK API。
- 不发布或自动下载任何预编译 Host、npm package、wheel 或 JAR。
- 不携带或安装 Node.js、Python、JDK/JRE 或 Rust toolchain。
- 不要求 TS/Python/Java 复制 Rust 生命周期、泛型单态化、`Arc<dyn Trait>`、
  `Pin<Box<Future>>`、过程宏展开或内存布局。
- 不把 workspace 子 crate 的内部 `pub` 项纳入对等承诺。
- 不在第一份协议中提供 TCP、HTTP、WebSocket 或公网服务；远程 transport 若未来需要，
  必须复用同一服务语义并另行决策。
- 首期不实现通用 ACP Client、Proxy、Conductor 或 EKO 对外部 ACP Agent 的产品接入。
- 不使用ACP draft v2或 unstable extension 代替稳定v1合同；未来采用须独立决策。
- 不把完整 facade 硬塞进标准 ACP字段；标准无法无损表达的能力只进入协商后的
  `_echo_agent/*` profile。
- 不保留旧协议或第二执行路径作为失败 fallback。

## 6. 系统边界与分层

```text
┌───────────────────────────────────────────────────────────────┐
│ Standard ACP Client │ TypeScript SDK │ Python SDK │ Java SDK │
│ ACP v1 only         │ ACP v1 + echo-agent extension profile  │
├───────────────────────────────────────────────────────────────┤
│ official ACP client SDK │ generated extension values/handles  │
├───────────────────────────────────────────────────────────────┤
│ ACP v1 JSON-RPC/stdio + negotiated _echo_agent/* methods      │
└──────────────────────────────┬────────────────────────────────┘
                               │ stdin/stdout
┌──────────────────────────────▼────────────────────────────────┐
│ echo-agent-sdk-host                                          │
│ official ACP Agent runtime │ ACP projection │ SDK extensions  │
├───────────────────────────────────────────────────────────────┤
│ echo_agent facade                                            │
│ AgentTurnDriver │ EventEnvelope │ RuntimeTaskService          │
│ Tool/MCP │ Store/Memory │ LlmClient │ Subagent │ Workflow     │
└───────────────────────────────────────────────────────────────┘
```

### 6.1 框架层

以下内容属于 `echo-agent`，因为任何复用该框架的非 Rust 消费者都需要：

- 根 facade 上可选的 `acp` feature 与 ACP Agent adapter；
- 对官方 ACP schema/runtime 的最小适配与 conformance；
- `_echo_agent/*` SDK扩展值、catalog与版本；
- SDK Host 生命周期与本地 transport；
- facade API 到 command/value/handle/extension 的无损适配；
- feature manifest 与公共 API parity manifest；
- 三语言 SDK 及其合同测试。

### 6.2 应用层

以下内容不进入 SDK Host：

- EKO workspace、UI字段、reviewer policy、worktree policy 和文件权威；
- GUI/TUI/CLI 投影与 Tauri command；
- EKO 特有的配置、会话列表和产品持久化布局。
- EKO 启动或发现外部 ACP Agent、选择ACP实现和向GUI/TUI投影ACP状态的产品策略。

`echo-agent-cli` 可以像其他消费者一样使用或验证 SDK，但不是 SDK 行为事实源。

## 7. 公共 API 权威与对等清单

### 7.1 权威集合

SDK 对等集合由以下条件共同确定：

1. 从根 package `echo_agent` 可达；
2. 对外 `pub` 且未使用 `#[doc(hidden)]` 隐藏；
3. 出现在正式 rustdoc/facade 模块中；
4. feature-gated API 记录其 feature 条件；
5. 过程宏与 derive 宏记录其对外行为，而不是记录展开后的 Rust 代码。

子 crate 中仅为实现复用而公开、但不从根 facade 承诺的类型，不进入集合。一个 API
是否被 EKO 使用不影响其是否进入集合。

### 7.2 Parity manifest

仓库维护机器可检查的 parity manifest。每个 facade 公共项必须归入且只能归入一类：

| 分类 | 含义 | 三语言责任 |
|---|---|---|
| `wire_value` | 可序列化值、枚举、错误、配置 | 生成或无损手写等价类型 |
| `operation` | 构造、查询、控制、纯函数 | 提供等价 SDK 方法或本地 helper |
| `handle` | 长生命周期对象或框架资源 | 提供不透明 handle 与生命周期 API |
| `stream` | 异步事件或数据流 | 提供语言惯用流与背压/取消语义 |
| `extension` | 消费者实现的 trait/callback | 提供反向 RPC adapter |
| `language_intrinsic` | 宏、泛型、所有权等语法机制 | 提供行为等价 helper/decorator/interface |

每项记录 Rust path、feature、语义分类、TS/Python/Java 映射、合同测试和状态。不存在
“暂不支持但仍宣称完整对等”的状态；无法映射时必须先改变 facade 公共承诺或补齐协议。

每项还必须声明一个 ACP relationship：

- `standard`：可由稳定 ACP v1 无损表达；
- `standard_projection`：标准ACP仅提供有界兼容视图，完整语义仍需SDK扩展；
- `echo_extension`：只由协商后的 `_echo_agent/*` 方法表达；
- `language_intrinsic`：由语言本地 helper/decorator/interface 表达，不进入wire。

ACP relationship 只是适配分类，不能成为第二套状态或完成度权威。

公共 API inventory 由固定工具链从 rustdoc/public-api 信息生成，parity manifest 与
inventory 的差异是阻断性验证失败，避免靠人工列表掩盖新增 API。

## 8. SDK 对象模型

三种语言共享相同概念，不共享相同拼写：

```text
SdkHost (ACP Agent)
  ├─ ACP capabilities + echo-agent extension capabilities
  ├─ AgentHandle
  │    ├─ SessionHandle
  │    │    └─ RunHandle
  │    └─ direct Execute RunHandle
  └─ ExtensionRegistration
```

- `SdkHost` 代表一个由 SDK 启动或连接的 Host 子进程。
- 标准 ACP Client 只观察 ACP Session 与 Prompt Turn；echo-agent SDK Client 在同一连接上
  协商完整handle与extension能力。
- 标准ACP profile使用Host启动配置提供的一个默认Agent定义；若未配置，
  `session/new`明确失败。完整SDK profile可以通过扩展创建和引用多个`AgentHandle`。
- `AgentHandle` 代表 Host 内一个具体 Agent 实例或不可变构造定义。
- `SessionHandle` 代表多轮上下文及其明确生命周期。
- `RunHandle` 代表一次有限执行，提供状态、事件、结果、取消和恢复入口。
- `ExtensionRegistration` 代表一个宿主语言实现的 Tool、Store、LlmClient、Hook、
  HumanLoopProvider、AgentFactory 或其他公开扩展点。

handle 只包含稳定 ID 和 generation，不暴露内存地址。Host 关闭、generation 替换或对象
释放后，旧 handle 必须返回 typed stale/closed error，不能静默指向新对象。

## 9. 语言惯用 API

| 语义 | TypeScript | Python | Java |
|---|---|---|---|
| 异步结果 | `Promise<T>` | `await` / coroutine | `CompletionStage<T>` |
| 流 | `AsyncIterable<T>` | `AsyncIterator[T]` | `Flow.Publisher<T>` |
| 可选值 | `T | undefined` | `T | None` | `Optional<T>` 或 nullable contract |
| 判别联合 | tagged union | sealed/dataclass model | sealed interface/record hierarchy |
| 取消 | `AbortSignal` | cancellation scope/task cancel | cancellation token/future cancel |
| Tool helper | `tool({...})` | `@tool` | annotation/builder/interface |
| 资源关闭 | `AsyncDisposable`/`close` | async context manager | `AutoCloseable` |

三语言 SDK 优先组合官方 ACP Client SDK，生成代码只覆盖 `_echo_agent/*` 扩展及其 facade
值。不得 fork ACP schema 或复制官方 session/prompt/update 类型。

语言 SDK 可以提供便利方法，但便利方法必须落到同一个 canonical operation；不得因语言
习惯另建具有不同默认值、重试、状态或终态判断的执行路径。

## 10. 协议合同

### 10.1 协议分层与通道纪律

Host 的基础 wire 完全遵循稳定 ACP v1：官方 schema 与 runtime 决定 JSON-RPC envelope、
request ID、method、capability、Session、ContentBlock、update 与 stop reason。项目不得用
自有 DTO 覆盖、收窄或修改标准 ACP 类型。

- stdin/stdout framing、并发 request/notification 和反向 Client method 由官方 ACP Rust
  SDK 实现；项目不维护第二个 JSON-RPC parser。
- stdout 只承载 ACP wire；日志、banner 和诊断写入 stderr。
- 标准 ACP request ID 遵循ACP schema。SDK稳定对象、Run、Event、operation和幂等身份
  使用扩展payload中的非空字符串字段，不能借用JSON-RPC request ID充当领域身份。
- reader loop 不执行业务 handler；ACP callback 或SDK extension等待期间必须继续分派其他
  request、response与notification。
- writer是stdout唯一所有者；业务等待不得持有writer lock。
- ACP不支持的frame、method和字段按官方错误合同处理；SDK扩展不能改变标准错误行为。

### 10.2 初始化与双 profile 协商

连接首先执行标准 ACP `initialize`。Host声明标准 `agentCapabilities`，并在其 `_meta` 下
发布 namespaced echo-agent capability，至少包括：

- SDK extension protocol version 与 contract/source digest；
- 编译时启用的 `echo_agent` 叶 feature set；
- handle、event replay、callback、structured output 等扩展capability；
- message、stream、callback concurrency 和 payload bounds。

标准 ACP Client 可以忽略该 `_meta` 并继续使用标准 profile。echo-agent SDK Client 必须
识别并校验它；缺少共同SDK extension version、digest、必需feature或capability时，完整
SDK初始化失败且不得创建扩展handle，但不能破坏同一Host的标准ACP合规性。

不启用SDK扩展的客户端不得调用 `_echo_agent/*`。Host对未知标准方法和未知扩展方法分别
返回ACP规定的method-not-found；不得猜测、fallback或隐式启用扩展。

### 10.3 标准 ACP operation families

标准profile至少映射稳定ACP v1的：

- `initialize`与capability协商；
- `session/new`、支持时的`session/load`和Session配置；
- `session/prompt`、`session/update`与prompt stop reason；
- `session/cancel`；
- permission/elicitation；
- Client声明时的filesystem和terminal方法；
- plan、tool-call、message、usage、mode与command update。

ACP Session 与 Prompt Turn 必须复用同一个内部Session/Run服务。标准 plan、tool status、
mode和stop reason是兼容投影，不反向定义TaskRun、Subagent或Run终态。

标准ACP Host在启动时接收产品无关的默认Agent构造配置。ACP `session/new`只能从该定义
创建Session，并将规范中的cwd、MCP声明和Client capability无损映射到framework调用；
它不能读取EKO配置或隐式选择另一个Agent。

### 10.4 echo-agent SDK extension families

ACP没有无损表达的完整facade能力进入协商后的 `_echo_agent/*` namespace：

- Agent构造、配置、能力快照和关闭；
- Session/Run handle、查询、等待、恢复与有界event replay；
- TaskRun/PlanTask/Subagent创建、更新、查询、执行和控制；
- Tool、LlmClient、Store、HumanLoopProvider、Hook/Callback、自定义Agent等extension注册、
  调用、取消和注销；
- MCP、A2A、memory、workflow、state、delivery、trace、eval/improve与其它feature操作。

扩展operation只调用现有framework service/trait。它不重新计算ready frontier、不推断
终态、不拥有重试或恢复规则，也不将不同Rust语义压成一个ACP字段。

### 10.5 路径、数值与投影边界

- 标准ACP路径按规范使用绝对UTF-8字符串；无法表示的本地路径返回明确ACP错误，不能
  损坏或替换字符后继续。
- 完整SDK使用 `_echo_agent/*` 的`WirePath`保留Unix bytes或Windows UTF-16及可选显示值。
- ACP schema中的数值遵循ACP定义；SDK extension中的u64/usize/sequence/revision使用无损
  表达，不能经过JavaScript不安全number。
- 标准ACP update只包含规范允许的有界视图。任何丢失的framework identity、状态、错误、
  artifact、cursor或recovery事实必须通过SDK扩展保留。

### 10.6 错误合同

标准方法返回标准 ACP/JSON-RPC error与stop reason。SDK扩展错误另含稳定code、message、
retryability、operation/domain identity和有界details，至少区分：

- ACP protocol/version/capability mismatch；
- SDK extension version/digest/capability mismatch；
- invalid request/config/value；
- feature unavailable；
- stale/closed handle；
- framework typed error；
- extension rejected/failed/timed out/disconnected；
- cancellation；
- Host shutting down/exited；
- event gap/replay unavailable；
- serialization/payload bound violation。

语言SDK将扩展code映射为语言异常/结果类型；不得解析标准ACP message或stop reason来猜测
完整framework状态。

## 11. 事件、结果与恢复

### 11.1 事件权威

Host 原样承接框架 `EventEnvelope` 的 version、identity、sequence、parent link、timestamp、
content hash 和 `AgentEvent`。语言 SDK 可以生成更便利的视图，但不得删除身份字段或用
SDK 本地序号替换框架序号。

同一个framework event最多产生两个视图：标准ACP Client接收规范允许的
`session/update`投影；echo-agent SDK Client还接收带完整`EventEnvelope`的
`_echo_agent/*`扩展notification。两者必须来自同一已接受事件，ACP投影器不得产生新
sequence、终态或Task/Subagent状态。

一个有限 Run 必须符合现有 exactly-one-terminal 合同。EOF、SDK断连或 Host 退出不能被
解释为成功；只有框架终态和对应 receipt 能完成 Run。

### 11.2 Replay 与背压

- 每个订阅者维护显式 cursor；重连从最后确认 cursor 之后继续。
- replay 必须有固定 watermark、数量/字节上限和明确 `event_gap`。
- 慢消费者不得无限扩大 Host 内存；达到界限时停止投递并返回 typed gap/backpressure
  结果，不得静默丢事件。
- Event 是增量事实，不替代 `getRun`/snapshot；消费者需要完整状态时按 watermark 查询。
- ACP `session/load` 只承担ACP Session恢复，不冒充完整event replay。cursor、retained
  floor、fixed watermark和`event_gap`属于SDK扩展合同。

### 11.3 Session 与进程恢复

- live Session 的所有权属于 Host generation。
- 使用 `InMemoryStore` 或未配置持久状态时，Host 退出后 Session 明确不可恢复。
- 使用框架持久 Store/EventJournal 时，恢复必须读取已提交事实和 cursor，不重新推导已
  固定的 Agent、资源、权限或 extension binding。
- extension binding 跨 Host 重启默认失效；SDK 必须按新 generation 重新注册，并在恢复
  执行前完成依赖检查。

## 12. Extension Bridge

### 12.1 统一模型

宿主语言实现的公开trait通过同一条ACP双向连接和 `_echo_agent/extension/*` 方法注册。
Host保存：

- extension kind、ID、generation；
- schema/capability snapshot；
- concurrency、timeout 与 cancellation contract；
- 对应 SDK connection ownership。

Host调用extension时发送namespaced反向request；SDK dispatcher调用本语言实现并返回
typed result。标准ACP Client method继续由官方ACP handler分派，不进入SDK extension
registry。反向调用失败不得触发内置实现fallback，除非原Rust API本身定义了相同的显式
fallback语义。

### 12.2 Trait 映射

- `Tool`：名称、描述、参数 JSON Schema、revision、modality 与 `ToolContext` 无损传递；
  返回完整 `ToolResult`，包括 artifact 与 typed failure。
- `LlmClient`：支持 non-stream 与 stream；stream 具有独立 identity、chunk、终态、取消和
  timeout，不以一次巨大 response 模拟流。
- `Store`：保留 namespace、key、search mode、limit、pagination 和原子语义；不得把
  semantic/hybrid search 降级为 keyword。
- `HumanLoopProvider`：请求保持 request identity；响应只能结算一次，断连形成明确失败。
- Hook/Callback：保持触发顺序、可见输入、修改/拒绝结果和 timeout。
- `AgentFactory`/自定义 Agent：构造与 execute/chat stream 分离，事件继续进入框架
  `EventEnvelope`，不能由 SDK自行伪造 Run 成功。

ACP `session/request_permission` 是标准Client交互投影，不替代通用
`HumanLoopProvider`扩展；标准ACP Client可处理规范权限请求，完整SDK还必须保留框架请求
identity、typed response与结算事实。

### 12.3 并发与死锁约束

- reader loop 不执行用户 callback；callback 在语言 SDK 的受控 executor 中运行。
- 每次 callback 都有 deadline 与 cancellation identity。
- callback 可以发起独立 SDK 调用，但不得重入同一个被调用对象的排他 mutation；此类
  重入返回 typed conflict，而不是互相等待。
- connection teardown 先关闭新调用 admission，再取消在途 callback，等待有界结算，
  最后释放 handle。

## 13. Feature 模型

Rust Cargo feature 是 Host 的编译时能力。语言 SDK API 覆盖完整 facade，但每个 operation
声明所需 feature：

- 根 facade 增加可选 `acp` feature并纳入`full`与公开feature矩阵；它依赖官方稳定ACP
  Rust SDK，不启用draft/unstable能力；
- ACP `initialize` 的namespaced `_meta` 返回实际编译 feature set；
- SDK 在发请求前可做快速 capability check；
- Host 仍是最终校验权威，缺失时返回 `feature_unavailable`；
- full parity 验证使用 `echo_agent/full`；
- 单 feature 验证确保 operation 不意外依赖未声明 feature；
- 测试、mock、eval 等公开 feature 同样进入 parity manifest，不因主要面向开发者而省略。

SDK 不模拟 Host 未编译的能力，也不自动切换到另一实现。

## 14. 生命周期与数据流

### 14.1 正常执行

```text
SDK builds/spawns Host as ACP Agent
  -> ACP initialize
  -> verify echo-agent extension capability/digest
  -> register required extensions
  -> ACP session/new or session/load
  -> _echo_agent Agent/Session/Run handles as needed
  -> ACP session/prompt or _echo_agent Run start
  -> receive ACP session/update + full EventEnvelope extension stream
  -> receive exactly one terminal + typed receipt
  -> close handles
  -> _echo_agent/runtime/shutdown (SDK profile) or stdin EOF (standard profile)
```

### 14.2 反向扩展调用

```text
AgentTurnDriver
  -> framework trait proxy
  -> Host sends _echo_agent/extension/invoke request over ACP connection
  -> SDK callback executor
  -> typed result/stream/error
  -> trait proxy
  -> existing framework state machine continues
```

### 14.3 SDK 主动取消

```text
SDK cancel
  -> ACP session/cancel or _echo_agent Run cancel
  -> Host resolves shared Session/Run identity and generation
  -> existing CancellationToken / service control
  -> in-flight extension cancellation
  -> framework emits canonical terminal
  -> Run receipt settles once
```

取消与自然完成竞争时，以框架现有终态/CAS 语义决定唯一结果；transport 不补写第二终态。

## 15. 异常与边界场景

| 场景 | 目标行为 |
|---|---|
| Host 无法编译或启动 | SDK返回 start error；不伪装为协议错误 |
| stdout 出现非ACP文本 | connection失败并指出framing violation；stderr保留诊断 |
| ACP v1不兼容 | 按ACP初始化失败，无fallback |
| 标准ACP Client不支持echo扩展 | 标准profile继续可用，但不得调用或展示完整SDK能力 |
| SDK Client未协商echo扩展 | 完整SDK初始化失败，不创建扩展handle |
| JSON-RPC数字request ID | 按ACP官方schema接受；领域identity仍使用独立字符串字段 |
| ACP绝对UTF-8路径无法表示本地路径 | 标准方法明确失败；SDK扩展使用无损WirePath |
| 必需 feature 缺失 | 返回 `feature_unavailable`，不创建部分对象 |
| SDK 进程/连接断开 | Host 关闭 admission、取消其所有 callback 与 owned live handles |
| Host 意外退出 | SDK 将未终结操作结算为 `host_exited`，绝不当成成功 |
| callback 超时 | 取消 callback，返回 typed timeout，由原框架策略决定是否重试 |
| callback 晚到响应 | generation/request 已结算，响应丢弃并记录诊断，不覆盖新状态 |
| 重复ACP request ID | 交由官方ACP runtime按协议处理，不赋予领域幂等语义 |
| 重复SDK operation/idempotency identity | 拒绝或返回同一幂等结果，取决于framework operation合同 |
| 取消与终态竞争 | 只接受框架权威的一个终态 |
| 消费者落后于 retained floor | 返回 `event_gap` 与可查询 snapshot watermark |
| 载荷超限 | 在分配/执行前拒绝；错误不回显敏感完整载荷 |
| Store/LLM/Tool extension 断连 | 返回对应 typed extension failure，无隐式内置替代 |
| Host stdin EOF | 视为 owner 离开，执行有界关闭和进程退出 |

## 16. 安全与本地边界

SDK Host 是开发者主动启动的本地子进程，不引入线上多租户、SSRF 或公网权限模型。
ACP stdin/stdout connection本身就是调用边界，不增加账号、token或full-auto权限门控。

仍需保留本地成立的防护：

- 密钥不进入协议诊断、stderr、事件 details 或 panic 文本；
- stdout 只允许协议，防止数据与日志混流；
- payload、队列、replay、callback concurrency 与等待时间全部有界；
- 工作目录、环境变量和文件路径由 SDK 显式传递，不从不可信事件文本执行；
- Host 退出清理完整子进程树和临时资源；
- 不允许 transport 绕过框架已有 permission、sandbox 和 cancellation policy。
- ACP Agent向Client发起filesystem、terminal、permission或elicitation方法时，只使用Client
  已协商capability；缺失capability时明确失败，不转向隐藏的本地执行路径。

这些约束防止框架 bug、资源耗尽和无意数据泄漏，不是面向公网租户的权限闸。

## 17. 源码布局与交付合同

目标布局：

```text
echo-agent/
├── src/acp/                 # 根facade稳定ACP v1 Agent adapter与投影
├── echo-sdk-protocol/       # _echo_agent/* 扩展DTO、版本、Schema export
├── echo-sdk-host/           # ACP Agent Host + echo-agent SDK扩展binary
├── contracts/sdk/
│   ├── schema/              # 官方ACP引用 + echo-agent扩展机器合同
│   ├── fixtures/            # ACP conformance与SDK扩展golden fixtures
│   └── parity-manifest.*    # facade inventory 到三语言的映射
├── sdks/
│   ├── typescript/
│   ├── python/
│   └── java/
└── docs/sdk/                # 唯一外部 SDK文档入口
```

开发者从同一个 Git tag/revision：

1. 用仓库声明的 Rust toolchain 构建 `echo-agent-sdk-host`；
2. 用语言 manifest 声明的工具链构建目标 SDK；
3. 将本地 Host 路径显式传给 SDK；
4. SDK 与 Host 先完成 ACP v1 initialize，再确认 echo-agent extension
   source/protocol/schema/feature compatibility。

仓库可以提供构建与验证脚本，但脚本只在开发者机器生成产物；项目不上传、下载或隐式
安装产物。

## 18. 版本与兼容策略

- Git revision 是源码交付的第一兼容边界。
- ACP wire protocol version、官方ACP crate/schema artifact version、echo-agent extension
  protocol version和crate/package source version相互独立；不得用其中一个推断其它版本。
- 标准ACP兼容性只由ACP `initialize.protocolVersion` 与capability决定。
- 完整SDK兼容性还要求namespaced extension version、contract digest和feature协商通过。
- additive wire field 使用向前兼容规则；未知事件保留为 typed `unknown` view，同时保存
  原始 type 与有界 payload，旧 SDK 不得崩溃。
- 删除字段、改变默认值、改变终态/重试/取消语义或复用既有错误 code 属于 breaking
  protocol change。
- 同一 echo-agent extension major 内，Host 与 SDK 必须协商可用 minor range 和 feature
  set；标准ACP方法继续遵守ACP自身兼容规则。
- 本项目处于开发期，不保留过时协议 fallback；一次变更应同步更新 Host、三语言 SDK、
  fixtures、parity manifest、示例和文档。

## 19. 文档与示例

`docs/sdk/README.md` 是唯一外部入口，并导航到：

- 源码构建与工具链前提；
- TypeScript、Python、Java quickstart；
- Agent/Session/Run 对象模型；
- 标准 ACP v1 profile、ACP Agent启动方式与受支持capability；
- `_echo_agent/*` 完整SDK profile及其与标准ACP的边界；
- streaming、cancellation、recovery、HITL；
- Tool/Store/LlmClient/Hook/Agent extension；
- feature compatibility；
- protocol/error reference；
- ACP protocol version、官方artifact version与echo-agent extension version兼容矩阵；
- parity manifest 与当前完成状态。

每项主要 facade 能力至少有一个 Rust 示例和三语言等价示例。示例进入真实编译/类型检查
与 Host E2E；代码片段仅展示而不编译不能作为对等证据。

`echo-website` 只在 SDK 达到源码可构建、三语言行为验证完成后增加公开入口；设计阶段不
提前宣传未实现能力。

## 20. 验收标准

### 20.1 公共面完整性

- 根 `echo_agent` all-features public inventory 与 parity manifest 零缺项。
- 每项 API 有唯一分类、三语言映射和验证引用。
- 每项API明确映射为ACP standard、standard projection、echo extension或language
  intrinsic，标准投影损失不得冒充完整对等。
- 新增 facade API 而未更新 SDK映射时，CI 阻断。

### 20.2 合同一致性

- 官方ACP Rust schema/runtime通过稳定v1 conformance；项目不维护分叉schema。
- Rust、TS、Python、Java 对同一echo-agent扩展golden
  request/event/result/error得到等价值。
- Schema 重新生成后工作树无未提交 diff。
- 字段 rename、optional/default、整数范围、timestamp、binary/reference payload 无损。
- unknown additive event 在三语言均可观察且不导致 stream 失败。
- 标准ACP Client忽略echo-agent extension后仍能完成initialize、session和prompt；完整SDK
  Client缺少协商扩展时fail closed。

### 20.3 行为一致性

- 三语言分别通过真实 Host 验证 Agent create、Session、execute/chat、stream、structured
  output、cancel、resume 和 close。
- 至少一个非echo-agent标准ACP Client通过真实Host验证稳定v1
  initialize、session/new、session/prompt、session/update和cancel。
- ACP permission/filesystem/terminal/plan/mode映射只在capability协商后启用，并投影同一
  framework执行事实。
- TaskRun、PlanTask、Subagent 的状态、依赖、重试、暂停/恢复与终态轨迹一致。
- Tool、LlmClient、Store、HumanLoopProvider、Hook/Callback、自定义 Agent extension
  分别通过成功、typed failure、timeout、cancel、disconnect 和 late-response 测试。
- ACP、MCP、A2A 与每个 facade feature 均有对应行为验收。

### 20.4 可靠性

- 一个 Run 恰好一个权威终态；EOF 与进程退出不产生假成功。
- 同一Run的ACP stop reason、session/update和完整SDK terminal receipt互相一致；任何
  profile都不能补写第二终态。
- Host 强杀、SDK 强杀、callback 慢/挂起、消费者背压、event gap、恢复和重复请求均有
  确定结果。
- shutdown 关闭 admission、取消在途工作、结算有界等待并回收完整进程树。
- stdout 零噪声，stderr 不泄漏密钥。

### 20.5 源码交付

- 在干净 checkout 中仅凭仓库文档可构建 Host 和三语言 SDK。
- 不依赖未提交文件、工作树绝对路径或本项目发布的预编译Host/SDK产物；第三方源码依赖
  可以通过锁定manifest解析，不要求vendor全部生态依赖。
- 每种语言 quickstart 在干净示例项目中从源码编译并运行。

### 20.6 状态声明

项目必须区分：

- **Design**：本设计已确认；
- **Contract**：协议、Schema、parity manifest 已存在并通过合同检查；
- **ACP conformant**：标准ACP v1 Client可执行受支持profile且conformance通过；
- **Runnable**：真实 Host 与某语言完整SDK扩展路径可执行；
- **Parity complete**：TS、Python、Java 对全 facade/all-features 全部通过；
- **Published**：本设计明确不提供 registry/binary 发布，因此不得用该状态描述源码交付。

只有 `Parity complete` 可以宣称“全部 Rust 公共能力对等”。

## 21. 关键取舍与后果

### 21.1 收益

- Rust 继续是唯一语义权威，避免四套 Agent 核心漂移。
- 进程边界隔离 native failure，并自然支持三种语言的异步事件模型。
- 标准ACP客户端无需echo-agent专用SDK即可调用公共编码Agent能力。
- 三语言SDK可以组合官方ACP Client库，只维护完整facade所需的echo-agent扩展。
- 源码优先避免项目承担平台二进制与 registry 发布矩阵。
- parity manifest 将“全部对等”从口号变成可检查合同。
- 新语言可以复用同一协议、fixtures 和 Host，而不改变框架核心。

### 21.2 成本

- 所有消费者必须安装 Rust toolchain 并编译 Host。
- 完整 facade 对等面很大，尤其是 trait callback、all-features 和恢复故障矩阵。
- 双向 callback transport 需要严谨处理 demultiplex、重入、取消和断连。
- 需要长期跟踪稳定ACP版本，并独立验证ACP conformance与完整SDK parity。
- 标准ACP投影与完整SDK视图必须共享权威但保持不同完成口径。
- 每个 facade API 变更都具有三语言同步成本。

### 21.3 接受的限制

- TS 仅面向允许启动本地进程的 Node.js 类环境，不直接支持浏览器/Edge。
- Python/Java 同样要求平台允许子进程。
- 首期Host只实现ACP Agent role；通用Client、Proxy、Conductor以及draft v2不在范围内。
- 标准ACP路径和计划状态存在表达边界；完整SDK通过协商扩展保持无损，不能改变ACP标准。
- 使用者自行构建的 Host feature set 可能小于完整 facade；此时缺失能力必须显式失败。
- Rust-specific syntax 只保证外部行为等价，不保证源码结构等价。

## 22. 当前状态

本文件描述完整目标状态。当前仓库尚未包含`acp` feature、ACP Agent adapter、
`echo-sdk-protocol`、`echo-sdk-host`、三语言SDK、parity manifest、ACP conformance或对应
E2E，因此当前状态仅为 **Design**，不是 Contract、ACP conformant、Runnable或Parity
complete。
