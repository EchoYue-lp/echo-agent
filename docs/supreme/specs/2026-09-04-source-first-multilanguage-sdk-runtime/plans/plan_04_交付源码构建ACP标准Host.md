---
schema_version: 3
supersedes: null
slug: 2026-09-04-source-first-multilanguage-sdk-runtime/plan
goal: 交付一个从当前仓库源码构建、以显式产品无关配置启动并通过官方 stdio transport 暴露稳定 ACP v1 标准 profile 的
  echo-agent SDK Host。
ships: 可从源码构建的 echo-agent-sdk-host，以产品无关默认 Agent 配置提供标准 ACP v1
  profile，标准客户端可完成会话、提示、更新、取消和有界关闭。
verify: 从当前源码构建的 echo-agent-sdk-host 可由官方 ACP Client 作为真实子进程启动，并通过 loopback
  模型服务完成 initialize、session/new、session/prompt、session/update、session/cancel 与
  stdin EOF 有界退出；ACP 模式 stdout 每行都是合法协议消息，错误配置非零退出且不泄漏凭据，./scripts/verify.sh
  与适用条件矩阵全部通过且零警告。
design_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md
delivery_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/plans/delivery-map.md#acp-standard-host
todos:
  - id: create-source-built-host
    files:
      - Cargo.toml
      - Cargo.lock
      - AGENTS.md
      - echo-sdk-host/Cargo.toml
      - echo-sdk-host/src/lib.rs
      - echo-sdk-host/src/config.rs
      - echo-sdk-host/src/main.rs
      - echo-sdk-host/config.example.json
    summary: 新增非发布的 echo-sdk-host workspace crate、显式版本化配置和 stdout-safe CLI 启动入口。
    verify: Host binary 可从源码独立构建；合法配置在连接 stdio 前完成解析和验证，非法参数、配置或凭据来源冲突非零退出且 stdout
      为空、错误有界且不含 secret。
  - id: build-default-session-agent
    files:
      - echo-sdk-host/src/lib.rs
      - echo-sdk-host/src/factory.rs
      - echo-sdk-host/src/mcp.rs
    summary: 复用 FrameworkConfig、LlmConfig、ReactAgent 与 AcpAgentAdapter，为每个 ACP
      Session 构造独立默认 Agent 并准备 stdio MCP。
    verify: 每个 session/new 获得独立 Agent、Session/Conversation/cwd identity；所有 stdio MCP
      声明在响应前连接，重复名称、相对或非 UTF-8 command、remote MCP、额外目录和不受支持的 memory/HITL
      配置明确失败，部分准备失败会清理已创建资源。
  - id: prove-real-stdio-host
    files:
      - echo-sdk-host/tests/stdio_e2e.rs
      - .github/workflows/rust-ci.yml
    summary: 用官方 ACP Client 启动真实 Host 子进程，验证标准流程、取消、stdout framing、错误配置和有界进程退出，并纳入 CI。
    verify: 源码构建 binary 在 loopback 模型服务上完成标准 ACP v1 主流程；活动 Prompt
      可取消且连接可结算，Client/EOF 后 Host 有界退出；所有 stdout line 可解析为 JSON-RPC，stderr
      与错误不泄漏 sentinel secret；Linux test 与 Windows binary compile 覆盖新 crate。
  - id: document-standard-host
    files:
      - README.md
      - README.zh.md
      - CHANGELOG.md
      - docs/adr/0028-source-first-multilanguage-sdk-runtime.md
      - docs/sdk/README.md
      - docs/sdk/protocol.md
      - docs/sdk/acp-agent-adapter.md
      - docs/sdk/acp-standard-host.md
    summary: 同步源码构建、配置、标准 profile、状态阶梯、限制和下一阶段扩展边界。
    verify: 文档给出可执行源码构建与配置路径，准确声明标准 Host 与 ACP conformant 证据，并保持语言 SDK、完整扩展
      profile、Runnable、Parity complete 和 Published 未完成；echo-website 继续不提前宣传。
artifact_id: plan:373c6e41-a875-4fac-9552-5d6e3fa9c602
lifecycle: completed
design_revision: sha256:9685cabd8f692ea060b67e46fe1c7d47531edb44efd998913e252803ef122817
---
## Approach

- 新建 `echo-sdk-host` workspace crate，package 保持 `publish = false`，binary 固定为 `echo-agent-sdk-host`。`src/main.rs` 只解析 CLI、初始化 stderr tracing、调用 library 并以有界错误退出；配置、Agent factory 与 MCP 转换留在可测试的 library。
- Host 只接受显式 `--config <path>`，不搜索 cwd、home、`.env` 或 EKO 配置。配置使用 1 MiB 上限的 schema v1 JSON，主体复用 `FrameworkConfig`，另以可选 `api_key_env` 解析 credential；inline token 与 env source 同时出现时拒绝。
- 启动时把 `ModelConfig` 解析为 `LlmConfig` 并真实 `build_client()`，fail fast 后共享一个 `Arc<dyn LlmClient>`。每次 `session/new` 将 cloned `FrameworkConfig` 转成新的 `AgentConfig`，注入 Session/Conversation/cwd，构造独立 `ReactAgent` 并复用 `FrameworkConfig::apply_compressor`。
- 默认 Session factory 只实现 adapter 已真实宣告的标准 profile。ACP stdio MCP 声明转换为框架 `McpServerConfig` 并在 `session/new` 响应前全部连接；未宣告的 HTTP/SSE MCP、additional directories、memory 与 human-loop 配置明确拒绝。
- `AcpAgentAdapter` 继续拥有 initialize/new/prompt/update/cancel、Session registry、Turn driver、背压和有界关闭。Host 只把 factory 连接到官方 `agent_client_protocol::Stdio::new()`，不新增 parser、reader loop、writer、Session 或终态状态机。
- 真实 E2E 用 `CARGO_BIN_EXE_echo-agent-sdk-host`、官方 `AcpAgentConfig`/`Client.builder()` 和 loopback OpenAI-compatible SSE fixture server；不增加 test-only Host 协议或真实公网/secret 依赖。

## Global Constraints

- 该 Host 属于通用 `echo-agent` 框架，与 `echo-agent-cli`、EKO workspace、GUI/TUI、产品持久化和产品权限策略无关。
- 只交付源码；不提交 binary/target，不执行或实现自动下载、`cargo install`、Node/Python/JDK/JRE/Rust toolchain 安装。
- `agent-client-protocol` 固定 `=2.1.0` 且不启用任何 `unstable_*` feature；stdio framing、JSON-RPC 与子进程进程组语义全部复用官方 SDK。
- ACP 运行模式的 stdout 只能由官方 Stdio writer 写协议；所有 tracing、配置错误和诊断写 stderr，不使用 banner 或 `println!`。
- 配置文件大小上限 1 MiB，`schema_version` 必须等于 1；model name、完整 endpoint、api protocol、Agent name/system prompt、positive max iterations 和 `enable_tools = true` 必须在打开 ACP stdio 前验证。
- 配置可使用 `default_agent.model.auth_token` 或 `api_key_env`，两者互斥；两者皆无时允许本地无认证 provider。Host 不 Debug/序列化/回显完整配置或 token。
- 每个 ACP Session 构造新 `ReactAgent`；同一个已验证 LLM client 可以跨 Session 共享，conversation history、MCP manager 与 working directory 不共享。
- 首期必须支持绝对 UTF-8 command 的 ACP stdio MCP；重复 server name、相对/non-UTF-8 command、HTTP/SSE MCP、非空 additional directories 在执行前失败。第 N 个 MCP 失败时关闭 Agent并清理前 N-1 个连接。
- `enable_memory` 与 `enable_human_in_loop` 在本标准 Host 阶段拒绝，直到对应持久化和 ACP Client callback 语义由后续独立 outcome 交付。
- 本阶段不发布 `_meta.echo_agent`、不实现 `_echo_agent/*` handler、语言 SDK、通用 ACP Client/Proxy/Conductor、remote Agent transport 或 EKO 外部 Agent 接入。
- 不修改根 `echo_agent` public facade；SDK public inventory/parity 和 extension schema 应保持零漂移。若代码事实迫使变化，停止并重新检查交付边界。
- 不使用 `unwrap`、`expect`、panic API、可能越界的直接索引或 UTF-8 字节截断；配置、stderr 与测试捕获全部有界。

## Files

- Modify: `Cargo.toml` — 将 `echo-sdk-host` 加入 workspace members/default-members。
- Modify: `Cargo.lock` — 记录新 workspace package 的现有依赖边。
- Modify: `AGENTS.md` — 更新完整 workspace 成员描述。
- Create: `echo-sdk-host/Cargo.toml` — 定义非发布 library 与 `echo-agent-sdk-host` binary，只依赖稳定 ACP、root acp+mcp 与最小 runtime/config/logging。
- Create: `echo-sdk-host/src/config.rs` — 定义 schema v1 Host 配置、显式文件加载、credential source 解析和 fail-fast validation。
- Create: `echo-sdk-host/src/factory.rs` — 复用现有 config/LLM/Agent 权威构造每 Session 独立 ReactAgent。
- Create: `echo-sdk-host/src/mcp.rs` — 无损转换并验证 ACP stdio MCP 声明，拒绝未宣告 transport。
- Create: `echo-sdk-host/src/lib.rs` — 组合配置、factory、adapter 与官方 Stdio 的公共 Host 入口。
- Create: `echo-sdk-host/src/main.rs` — 实现 `--config`、`--check-config`、`--help`、`--version` 和 stderr-only failure exit。
- Create: `echo-sdk-host/config.example.json` — 可由测试解析的 source-only 示例配置，不包含真实凭据。
- Create: `echo-sdk-host/tests/stdio_e2e.rs` — 真实 binary/loopback provider/official ACP Client 主流程、取消、EOF、stdout 与错误配置测试。
- Modify: `.github/workflows/rust-ci.yml` — Linux 测试矩阵加入独立 SDK Host group，Windows 增加 native binary compile。
- Modify: `README.md` — 增加标准 Host 源码构建入口与当前完成度。
- Modify: `README.zh.md` — 同步中文说明。
- Modify: `CHANGELOG.md` — 记录标准 Host、配置和真实子进程 conformance。
- Modify: `docs/adr/0028-source-first-multilanguage-sdk-runtime.md` — 记录 Host 实际配置/factory/stdio/MCP/关闭边界。
- Modify: `docs/sdk/README.md` — 更新 status ladder 与 Host入口。
- Modify: `docs/sdk/protocol.md` — 记录标准 Host wire、配置和 stdout/stderr纪律。
- Modify: `docs/sdk/acp-agent-adapter.md` — 衔接 adapter 与默认 Host factory。
- Create: `docs/sdk/acp-standard-host.md` — 提供源码构建、配置、启动、支持方法/MCP、错误与退出参考。

## Reuse

- `src/acp/adapter.rs` — `AcpAgentAdapter` — 唯一标准 ACP handler、Session/Turn/cancel/update/backpressure/shutdown权威。
- `src/acp/session.rs` — `AcpSessionFactory` / `AcpSessionContext` — Host 默认 Agent 的既有 Session 构造边界。
- `src/config.rs:15` — `FrameworkConfig` — product-neutral 默认 Agent 定义，不新建重复配置字段。
- `src/config.rs:30` — `From<FrameworkConfig> for AgentConfig` — 复用完整 model/Agent/tool/budget/timeout 映射。
- `src/config.rs:73` — `FrameworkConfig::apply_compressor` — 复用压缩策略安装。
- `echo-integration/src/providers/config.rs:103` — `LlmConfig::for_provider` / `build_client` — 复用 endpoint、protocol、thinking 与 provider client 验证。
- `src/agent/react/mod.rs:353` — `ReactAgent::new` — 从 canonical AgentConfig 构造每 Session Agent。
- `src/agent/react/capabilities.rs:1473` — `connect_mcp_from_config` — 复用 MCP lifecycle 和 Tool 注册。
- `echo-integration/src/mcp/server_config.rs` — `McpServerConfig` — ACP stdio MCP 到框架 transport 的唯一目标类型。
- 官方 `agent_client_protocol::Stdio` — 复用 newline JSON-RPC transport，确保 stdout writer 单一。
- 官方 `AcpAgentConfig` / `AcpAgent` — 真实 E2E 复用结构化子进程启动、stderr capture 与进程组清理。
- `.github/workflows/rust-ci.yml` — 保持现有 Linux 分组测试和 Windows compile 结构，不恢复单一 OOM workspace job。

## Todos

### create-source-built-host

requirements:
- § 5.1 范围
- § 5.2 非目标
- § 6.1 框架层
- § 10.1 协议分层与通道纪律
- § 16. 安全与本地边界
- § 17. 源码布局与交付合同

interfaces:
- consumes: `FrameworkConfig`、官方 stable ACP runtime、workspace source revision。
- produces: `echo-sdk-host` crate、`SdkHostConfig` schema v1、`HostCli`、`run_stdio` 与 `echo-agent-sdk-host` binary。

steps:

1. 新增 publish=false Host crate并注册 workspace；保持根 facade、extension contract 与语言 SDK package 边界不变。
   verify: workspace metadata、package targets 与 dependency feature tree。
   expected: `echo-sdk-host` 同时提供可测试 library和固定名 binary，依赖中没有 unstable ACP、预编译产物下载器或语言 runtime。
2. 实现显式 CLI和1 MiB有界JSON配置读取；复用 `FrameworkConfig`，解析互斥的 inline/env credential并在stdio启动前验证全部必需字段与本阶段不支持配置。
   verify: valid/example、unknown field、schema version、缺model/endpoint/protocol、credential冲突、memory/HITL/tools等正反例。
   expected: 合法配置产生已验证且不暴露secret的运行值；非法配置非零退出，stdout为空，stderr有界且不包含token。
3. 初始化 `RUST_LOG`兼容的stderr tracing并把main保持为薄入口；ACP运行直接await library `run_stdio`。
   verify: help/version/check-config与默认ACP运行的输出通道。
   expected: ACP模式stdout没有日志/banner；非ACP帮助命令行为明确；stdin EOF由官方Stdio传递给adapter关闭。

### build-default-session-agent

requirements:
- § 4.1 已有通用权威
- § 10.3 标准 ACP operation families
- § 13. Feature 模型
- § 14.1 正常执行
- § 15. 异常与边界场景

interfaces:
- consumes: validated `SdkHostConfig`、shared `Arc<dyn LlmClient>`、`AcpSessionContext`、`McpServerConfig`、`AcpAgentAdapter`。
- produces: `DefaultHostSessionFactory`与 ACP stdio MCP converter，返回独立 `Box<dyn Agent>`。

steps:

1. 启动时用 `LlmConfig::for_provider`与`build_client`解析并验证模型endpoint/protocol/credential，只共享无会话状态的 LLM client。
   verify: local/no-auth与env/inline credential构造、非法endpoint fail-fast、Debug/error不含secret。
   expected: Host不会进入ACP循环后才发现缺失或不可构造的LLM client。
2. 每次Session把FrameworkConfig转换为新AgentConfig，注入稳定Session ID、conversation ID和cwd，创建ReactAgent并调用既有compressor安装。
   verify: factory字段/identity/cwd测试与两Session实例隔离。
   expected: 两个Session不共享Agent/context/MCP状态，但共享provider client不会改变对话语义。
3. 校验并转换全部ACP stdio MCP declarations，连接成功后才返回Agent；拒绝重复、相对/non-UTF-8 command、remote transport与additional roots，任何部分失败都执行Agent close。
   verify: converter字段级测试、拒绝场景与部分连接失败资源清理。
   expected: 标准stdio MCP满足ACP合同；`session/new`成功意味着全部所需MCP已准备，失败不遗留子进程。
4. 用DefaultHostSessionFactory构造AcpAgentAdapter并连接官方Stdio；Host不重新拥有Session、Turn、event或terminal逻辑。
   verify: 调用路径检查与typed integration。
   expected: Host只提供配置/工厂/transport胶水，所有标准ACP行为继续由已审adapter产生。

### prove-real-stdio-host

requirements:
- § 10.1 协议分层与通道纪律
- § 14.3 SDK 主动取消
- § 15. 异常与边界场景
- § 20.2 合同一致性
- § 20.3 行为一致性
- § 20.4 可靠性
- § 20.5 源码交付

interfaces:
- consumes: source-built binary、official `AcpAgentConfig`/`Client.builder()`、loopback OpenAI-compatible SSE server。
- produces: `stdio_e2e`行为证据、stdout/stderr/exit约束与CI跨平台覆盖。

steps:

1. 从Cargo提供的真实binary路径，以结构化AcpAgentConfig启动Host；loopback模型服务返回确定性stream，不使用公网或真实secret。
   verify: official Client typed initialize → session/new → prompt → updates → EndTurn。
   expected: binary、配置、provider、ReactAgent、adapter和stdio均在真实主路径可达，Client能观察确定性消息。
2. 让loopback模型请求挂起，发送session/cancel并确认Cancelled；Client结束与直接stdin EOF均在timeout内结算Host和进程。
   verify: active Prompt cancel、connection continue/close、EOF exit与无orphan process。
   expected: 取消不是error或假成功；Host关闭沿用adapter有界清理，官方AcpAgent负责进程组，不自写kill模型。
3. 用AcpAgent debug捕获每个Host stdout line并逐条解析JSON-RPC；直接Command覆盖错误配置、sentinel secret和stderr上限。
   verify: stdout零非协议文本，malformed/missing配置非零退出，stdout为空，stderr可操作且无secret。
   expected: 标准Client不会因日志破坏framing，配置失败不会泄漏credential。
4. 将Host测试作为独立Linux CI group，并给Windows增加binary compile，保持现有runner资源分组。
   verify: workflow job/矩阵覆盖与本地workflow语法检查。
   expected: Linux执行真实子进程E2E，Windows至少证明native Host binary可编译，不扩大为OOM单一workspace job。

### document-standard-host

requirements:
- § 18. 版本与兼容策略
- § 19. 文档与示例
- § 20.5 源码交付
- § 20.6 状态声明

interfaces:
- consumes: source build命令、config.example.json、真实Host E2E和当前status ladder。
- produces: `docs/sdk/acp-standard-host.md`唯一Host参考与同步README/protocol/ADR/changelog。

steps:

1. 写Host文档和被测试解析的example配置，说明Rust工具链前提、源码构建、显式路径启动、credential、支持ACP/MCP面、stdout/stderr与退出。
   verify: 文档命令/路径与Cargo targets、配置schema、测试fixture一致。
   expected: 开发者可从同revision自行构建并启动Host，不会误以为SDK携带binary或语言runtime。
2. 更新SDK入口、协议、adapter衔接、ADR、README与CHANGELOG，依据真实E2E推进状态声明。
   verify: 状态文案与已通过证据逐项对应。
   expected: 标准Host可用且ACP conformant可以声明；语言SDK/full extension尚未交付，因此Runnable、Parity complete、Published保持未完成。
3. 运行contract drift检查并确认root public inventory、parity manifest、ACP baseline和extension schema零变化；记录examples与website影响。
   verify: `./scripts/check-sdk-contracts.sh`和最终diff。
   expected: Host crate不是root facade公共面；既有learning adapter示例继续有效，Host binary+example config+E2E构成本阶段可执行示例；echo-website按delivery map继续不修改。
4. 执行仓库全量门禁、适用条件矩阵和Host source build/E2E。
   verify: `./scripts/verify.sh`、AGENTS.md条件矩阵与Host focused命令。
   expected: 所有命令退出0、零warning、零fmt diff、零contract drift，不依赖未提交文件或项目预编译产物。

## Decisions

- 分层选择：SDK Host、default Agent factory、stdio/MCP适配属于通用框架源码；EKO发现、产品配置、GUI/TUI投影与产品权限策略继续留在echo-agent-cli。
- 重复性搜索：仓库没有生产Host、config loader或Host main；`demo72_acp_agent_adapter`仅是教学Agent。Host复用AcpAgentAdapter、FrameworkConfig、LlmConfig、ReactAgent和MCP manager，不复制其状态机。
- 配置选择显式版本化JSON文件而不是自动环境/目录发现。它可审阅、可测试且适合子进程启动；credential可显式引用一个env变量，避免隐含provider-specific env矩阵。
- 共享LLM client、逐Session新Agent：provider client是无会话transport，Agent持有history/cwd/MCP；该边界兼顾资源成本与Session隔离。
- 依据ACP官方Transports，stdio由Client启动Agent，newline JSON-RPC只走stdout，日志只走stderr；实现直接使用官方Stdio，不自建framing。
- 真实子进程测试依据官方Rust SDK的AcpAgentConfig/AcpAgent和Client builder；进程组、stderr retention与child grace period复用官方实现，不在本项目维护第二套。
- 延后memory/HITL/additional roots/HTTP-SSE MCP，因为当前adapter没有宣告对应capability或callback/persistence合同；标准stdio MCP在本阶段完成。
- `echo-website`不修改，因为delivery map要求等三语言Parity complete后再增加公开入口；不新增learning demo，因为本阶段真实可执行示例就是被E2E从源码构建并启动的Host binary。