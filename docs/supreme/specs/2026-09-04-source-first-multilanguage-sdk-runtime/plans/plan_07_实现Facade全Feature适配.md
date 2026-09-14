---
schema_version: 3
supersedes: null
slug: 2026-09-04-source-first-multilanguage-sdk-runtime/plan
goal: 在已交付的 ACP core profile 与双向 extension bridge 上，为根 echo_agent facade 的全部公开
  feature 建立可执行、可协商且无重复权威的 Host 适配层。
ships: SDK Host 为根 echo_agent facade 的
  Task、Subagent、workflow、state、delivery、trace、eval、improve、MCP、A2A及全部 feature
  提供无重复权威的完整适配。
verify: 从当前源码构建的 full-facade Host 可通过官方 ACP Client 调用每个 canonical facade adapter
  family，Task/Subagent/stream/handle/feature/cancel/close 语义均来自现有 Rust 权威；每个
  root facade 项恰好映射到标准/core、ExtensionBridge、Host operation 或 language
  intrinsic，alias 不重复注册，缺失 feature、错误 signature/handle/generation 与未知 operation
  全部 fail closed；standard/core/bridge-only Host 回归不变，合同漂移检查、./scripts/verify.sh
  与适用 root/Host feature 矩阵全部通过。
design_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md
delivery_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/plans/delivery-map.md#facade-feature-adapters
todos:
  - id: freeze-executable-facade-catalog
    files:
      - echo-sdk-protocol/src/inventory.rs
      - echo-sdk-protocol/src/facade.rs
      - echo-sdk-protocol/src/capability.rs
      - echo-sdk-protocol/src/error.rs
      - echo-sdk-protocol/src/handle.rs
      - echo-sdk-protocol/src/methods.rs
      - echo-sdk-protocol/src/catalog.rs
      - echo-sdk-protocol/src/schema.rs
      - echo-sdk-protocol/src/lib.rs
      - echo-sdk-protocol/tests/facade_inventory.rs
      - echo-sdk-protocol/tests/core_rpc_contract.rs
      - echo-sdk-protocol/tests/extension_contract.rs
      - contracts/sdk/facade-operation-catalog.json
      - contracts/sdk/parity-manifest.schema.json
      - contracts/sdk/parity-manifest.json
      - contracts/sdk/schema/echo-agent-extension-v1.schema.json
      - contracts/sdk/fixtures/extension/v1/
      - contracts/sdk/source-contract.json
    summary: 把启发式 facade 映射收敛为 canonical、可执行且可机械对账的 adapter catalog。
    verify: 每个根 facade 项恰好归入一个 canonical route；re-export alias 共享 source
      identity、signature、feature 与 handler，所有远程 operation
      都有精确输入/结果/handle/stream 合同和真实验证引用。
  - id: build-facade-runtime-and-feature-topology
    files:
      - echo-sdk-host/Cargo.toml
      - echo-sdk-host/src/features.rs
      - echo-sdk-host/src/config.rs
      - echo-sdk-host/src/lib.rs
      - echo-sdk-host/src/core_profile/mod.rs
      - echo-sdk-host/src/core_profile/state.rs
      - echo-sdk-host/src/core_profile/handles.rs
      - echo-sdk-host/src/core_profile/handler.rs
      - echo-sdk-host/src/core_profile/wire.rs
      - echo-sdk-host/src/core_profile/facade/mod.rs
      - echo-sdk-host/src/core_profile/facade/registry.rs
      - echo-sdk-host/src/core_profile/facade/stream.rs
    summary: 建立连接级 facade resource/stream runtime 与和根 crate 对齐的 Host feature 拓扑。
    verify: Host 对 compiled root leaf features 的 advertisement 与 Cargo
      激活集合一致；FacadeResource/Stream handle 受 generation、type、owner、bound、cancel 和
      close 约束，minimal/core/bridge Host 不会误开放 facade capability。
  - id: wire-task-subagent-structured-authorities
    files:
      - echo-sdk-protocol/src/methods.rs
      - echo-sdk-protocol/src/catalog.rs
      - echo-sdk-protocol/src/schema.rs
      - echo-sdk-host/src/factory.rs
      - echo-sdk-host/src/core_profile/state.rs
      - echo-sdk-host/src/core_profile/handles.rs
      - echo-sdk-host/src/core_profile/handler.rs
      - echo-sdk-host/src/core_profile/facade/task.rs
      - echo-sdk-host/src/core_profile/facade/subagent.rs
      - echo-sdk-host/src/core_profile/facade/structured_output.rs
      - echo-sdk-host/tests/facade_feature_adapters_e2e.rs
    summary: 将 TaskGraph、Subagent 与 structured output 方法接到 Session Agent 的既有服务权威。
    verify: RPC 与 Agent 内置工具观察同一个 TaskRevisionService/RuntimeTaskService revision 和
      DAG 终态；Subagent dispatch/await/message/guidance/interrupt/cancel 复用同一
      SubagentExecutor；structured output 不产生第二条 Agent 执行路径。
  - id: implement-stateful-facade-families
    files:
      - echo-sdk-protocol/src/facade.rs
      - echo-sdk-protocol/src/methods.rs
      - echo-sdk-protocol/src/schema.rs
      - echo-sdk-host/src/core_profile/facade/memory.rs
      - echo-sdk-host/src/core_profile/facade/workflow.rs
      - echo-sdk-host/src/core_profile/facade/state_delivery.rs
      - echo-sdk-host/src/core_profile/facade/observability.rs
      - echo-sdk-host/tests/facade_feature_adapters_e2e.rs
    summary: 适配 memory、workflow、state、delivery、trace、eval 与 improve 的状态和流式能力。
    verify: 每个 family 的构造、调用、stream、冲突、恢复、取消和关闭通过真实
      Host；Store/EventJournal/RuntimeStateStore/DeliveryLedger/RunStore/Workflow/Eval
      仍自行决定事务、序列、终态与恢复。
  - id: complete-optional-feature-and-extension-routes
    files:
      - echo-sdk-protocol/src/inventory.rs
      - echo-sdk-protocol/src/facade.rs
      - echo-sdk-protocol/src/methods.rs
      - echo-sdk-protocol/src/catalog.rs
      - echo-sdk-protocol/src/schema.rs
      - echo-sdk-host/src/core_profile/extension_bridge.rs
      - echo-sdk-host/src/core_profile/facade/integrations.rs
      - echo-sdk-host/src/core_profile/facade/tools.rs
      - echo-sdk-host/tests/facade_feature_adapters_e2e.rs
      - contracts/sdk/facade-operation-catalog.json
      - contracts/sdk/parity-manifest.json
    summary: 补齐 MCP、A2A 与其余叶 feature operation，并关闭所有尚未映射的公开 extension trait。
    verify: MCP/A2A 保留各自协议语义；其余叶 feature 各有真实成功和 feature-unavailable 路径；每个消费者实现
      trait 要么通过现有 ExtensionBridge typed kind/operation 调用，要么有经合同证明的
      language-intrinsic 映射，不存在悬空 extension。
  - id: prove-full-facade-and-document-status
    files:
      - echo-sdk-protocol/tests/facade_inventory.rs
      - echo-sdk-protocol/tests/core_rpc_contract.rs
      - echo-sdk-protocol/tests/extension_contract.rs
      - echo-sdk-host/tests/facade_feature_adapters_e2e.rs
      - echo-sdk-host/tests/core_profile_e2e.rs
      - echo-sdk-host/tests/extension_bridge_e2e.rs
      - echo-sdk-host/tests/stdio_e2e.rs
      - echo-sdk-host/config.sdk.example.json
      - scripts/check-sdk-contracts.sh
      - .github/workflows/rust-ci.yml
      - docs/sdk/facade-feature-adapters.md
      - docs/sdk/README.md
      - docs/sdk/protocol.md
      - docs/sdk/sdk-core-profile.md
      - docs/sdk/sdk-extension-bridge.md
      - docs/sdk/acp-standard-host.md
      - docs/adr/0028-source-first-multilanguage-sdk-runtime.md
      - README.md
      - README.zh.md
      - CHANGELOG.md
    summary: 用合同、真实 Host、单 feature 与文档门禁证明 facade adapter outcome 完整闭环。
    verify: full-facade 真实 Host 覆盖全部 family，minimal 与单 feature Host
      精确拒绝未编译能力，standard/core/bridge 回归、stdout/secret/teardown 边界、合同生成、CI
      分组和状态文档全部一致；三语言仍不宣称 Runnable 或 Parity complete。
artifact_id: plan:db3ce810-daa6-4751-9725-e2c107a51fac
lifecycle: completed
design_revision: sha256:10a237f834b9fb9cc8ea2d740d19222b0b5776fb30904e9b8b2df88f12b63227
---
## Approach

- 用唯一的 FacadeAdapterCatalog 收敛当前 parity manifest 的路径启发式映射。Catalog 按 canonical source identity、signature digest、feature semantics 和 handler family 建立一次路由；re-export 只保留 alias，不注册第二个 handler。
- echo-sdk-protocol 只负责 versioned wire、catalog、Schema、fixture 和生成合同。Facade operation 使用精确 typed record、receiver/resource type、result/stream descriptor 与稳定错误，禁止运行时反射 Rust 签名。
- echo-sdk-host 继续复用同一个 SdkCoreProfile、CoreProfileState、HandleRegistry、AcpConnectionServices 和官方 ACP Builder。新增 registry/resource/stream 只管理寻址、owner、generation、bounds 和 close，业务状态仍由 Rust framework service 持有。
- Task/PlanTask 共享 Session Agent 的 TaskRevisionService/RuntimeTaskService；Subagent 共享同一 SubagentRegistry/SubagentExecutor/event/control authority；structured output 复用既有 Agent/Run 路径。不得复制 DAG、Run、retry、terminal 或 callback transport。
- memory、workflow、state、delivery、trace、eval、improve、MCP、A2A 及叶 feature 通过显式 family adapter 调用既有服务。消费者实现的 trait 继续复用 ExtensionBridge；只有纯 Rust builder/marker/process-local 项才可有证据地归为 language intrinsic。
- Host Cargo feature 是能力权威：新增 facade 基础、all 和 root leaf passthrough，并用 root Cargo metadata 生成实际 advertisement。缺失 feature、unknown operation、错误 signature/receiver/type/generation 和资源越界全部 fail closed。
- 本计划只交付 Rust Host facade adapter 与合同/验证，不实现三语言 SDK，不修改 echo-agent-cli、echo-website 或 Skill。

## Global Constraints

- 能力属于通用 echo-agent framework；EKO 产品策略、GUI/TUI、workspace policy、应用持久化和 CLI SQLite 约束不进入本计划。
- Rust facade、TaskRevisionService、RuntimeTaskService、SubagentExecutor、Workflow、RuntimeStateStore、EventJournal、DeliveryLedger、RunStore、EvalRunner、Improve、MCP 和 A2A 各自保持唯一语义权威。
- 基础 wire 固定为 ACP v1、agent-client-protocol 2.1.0 和 schema 1.7.0；禁止 draft v2、自建 JSON-RPC parser/writer、第二连接或标准方法改写。
- 所有 _echo_agent 方法必须经过 initialize meta 协商；plain/unnegotiated Client 保持 method-not-found；只有真实编译且 catalog/handler/测试齐备的 feature 才 advertisement。
- Catalog 是 executable route 唯一事实源；parity manifest 继续覆盖每个 root facade item，但 alias 不得重复注册，禁止 wildcard、悬空 operation 和按路径猜执行语义。
- WireValue 只能出现在 catalog 明确允许的 typed leaf、metadata 或 provider raw value；u64/usize/revision/sequence/path/binary/timestamp 保持无损表示。
- Task create/update/execute/control 遵循 revision/CAS/claim/retry/terminal 权威；Subagent control 只使用真实 message、guidance、interrupt、cancel、await 语义，项目内部只使用 Subagent 术语。
- FacadeResource/Stream handle 遵循 shape、kind、generation、issued/closed、canonical type、owner 校验顺序；数量、payload、page、queue、等待和 shutdown 全部有界且幂等。
- MCP、A2A、LSP、channels 和网络验收只使用 loopback、mock process 或仓库 fixture；不访问公网、不添加本地桌面权限闸。
- eval+improve、channels+mcp 等 all-of 条件必须逐项满足；Host advertisement、preflight 和 handler 使用同一 feature semantics。
- full facade、minimal/core/bridge profile、每个 root leaf feature 和 all-of feature 均从源码验证；不提交、下载或隐式安装预编译 Host、npm、wheel、JAR 或任何语言 runtime。
- 生产 Rust 禁止 unwrap、expect、panic/unreachable、可能越界索引和 UTF-8 字节截断；stdout 只承载 ACP wire，stderr/typed error 不泄露密钥或完整敏感 payload。
- echo-website 在三语言 parity closeout 前不修改；本计划完成后只声明 Rust Host facade adapter available，不声明三语言 Runnable、Parity complete 或 Published。

## Files

- Modify: `echo-sdk-protocol/src/inventory.rs` — 以 canonical source/alias 和显式 route 生成 executable catalog。
- Create: `echo-sdk-protocol/src/facade.rs` — 定义 route、feature、input/result、resource/stream descriptor 和 catalog 校验。
- Modify: `echo-sdk-protocol/src/capability.rs` — 增加 facade capability、feature set 和 resource/stream limits。
- Modify: `echo-sdk-protocol/src/error.rs` — 增加 operation/signature/receiver/type/feature/resource failure 数据。
- Modify: `echo-sdk-protocol/src/handle.rs` — 增加 FacadeResource handle kind。
- Modify: `echo-sdk-protocol/src/methods.rs` — 将 Task/Subagent/Feature DTO 收敛为可执行 typed RPC。
- Modify: `echo-sdk-protocol/src/catalog.rs` — 绑定 family method、方向、capability 和 schema。
- Modify: `echo-sdk-protocol/src/schema.rs` — 生成 facade catalog、schema、digest 和 fixtures。
- Modify: `echo-sdk-protocol/src/lib.rs` — 导出 facade 合同模块。
- Modify: `echo-sdk-protocol/tests/facade_inventory.rs` — 验证 canonical/alias/handler/feature coverage。
- Modify: `echo-sdk-protocol/tests/core_rpc_contract.rs` — 验证 typed facade/Task/Subagent RPC 和 fail-closed。
- Modify: `echo-sdk-protocol/tests/extension_contract.rs` — 验证 ExtensionBridge obligation。
- Create: `contracts/sdk/facade-operation-catalog.json` — 保存生成的 canonical operation 输入。
- Modify: `contracts/sdk/parity-manifest.schema.json` — 增加 canonical route/alias/feature obligation。
- Modify: `contracts/sdk/parity-manifest.json` — 重生成逐项 facade 映射。
- Modify: `contracts/sdk/schema/echo-agent-extension-v1.schema.json` — 保存 typed facade 合同。
- Modify: `contracts/sdk/fixtures/extension/v1/` — 增加 family、signature、feature、resource、stream 正反样例。
- Modify: `contracts/sdk/source-contract.json` — 纳入 catalog digest。
- Modify: `echo-sdk-host/Cargo.toml` — 增加 facade 基础、all 和 root leaf passthrough features。
- Create: `echo-sdk-host/src/features.rs` — 生成 compiled feature set 并与 root metadata 对账。
- Modify: `echo-sdk-host/src/config.rs` — 增加 facade resource/stream/page/operation limits。
- Modify: `echo-sdk-host/src/lib.rs` — 组合 facade profile source-build 入口。
- Modify: `echo-sdk-host/src/core_profile/mod.rs` — 挂载 facade handler 到同一 ACP Builder。
- Modify: `echo-sdk-host/src/core_profile/state.rs` — 持有 SessionFacadeRuntime 和 adapter registry。
- Modify: `echo-sdk-host/src/core_profile/handles.rs` — 管理 Task/PlanTask/Subagent/FacadeResource/Stream ownership。
- Modify: `echo-sdk-host/src/core_profile/handler.rs` — 执行统一 admission、catalog、feature、handle 和 payload 梯子。
- Modify: `echo-sdk-host/src/core_profile/wire.rs` — 转换 framework facade 值、结果和错误。
- Create: `echo-sdk-host/src/core_profile/facade/mod.rs` — 组装显式 family adapters。
- Create: `echo-sdk-host/src/core_profile/facade/registry.rs` — 实现静态 route 和 resource registry。
- Create: `echo-sdk-host/src/core_profile/facade/stream.rs` — 实现有界 stream/page/cancel/close。
- Modify: `echo-sdk-host/src/factory.rs` — 注入 SessionFacadeRuntime、Task service 和 Subagent sidecar。
- Create: `echo-sdk-host/src/core_profile/facade/task.rs` — 适配 TaskRevisionService/RuntimeTaskService。
- Create: `echo-sdk-host/src/core_profile/facade/subagent.rs` — 适配 SubagentExecutor/control/event。
- Create: `echo-sdk-host/src/core_profile/facade/structured_output.rs` — 复用 structured output。
- Create: `echo-sdk-host/src/core_profile/facade/memory.rs` — 适配 Store/Conversation/Compression。
- Create: `echo-sdk-host/src/core_profile/facade/workflow.rs` — 适配 Workflow/Graph/SharedState/checkpoint。
- Create: `echo-sdk-host/src/core_profile/facade/state_delivery.rs` — 适配 RuntimeStateStore/EventJournal/DeliveryLedger。
- Create: `echo-sdk-host/src/core_profile/facade/observability.rs` — 适配 RunStore/TraceAnalyzer/Eval/Improve。
- Modify: `echo-sdk-host/src/core_profile/extension_bridge.rs` — 为剩余 consumer trait 补 typed kind/proxy 或完成重分类。
- Create: `echo-sdk-host/src/core_profile/facade/integrations.rs` — 适配 MCP/A2A/LSP/channels/telemetry/topology。
- Create: `echo-sdk-host/src/core_profile/facade/tools.rs` — 适配 web/files/shell/git/database/rag/chart/media/data/statistics/research/content-guard/project-rules/testing。
- Create: `echo-sdk-host/tests/facade_feature_adapters_e2e.rs` — 官方 Client + 真实 Host 全 family 验收。
- Modify: `echo-sdk-host/tests/core_profile_e2e.rs` — 保持 core-only 回归。
- Modify: `echo-sdk-host/tests/extension_bridge_e2e.rs` — 回归新增 trait route。
- Modify: `echo-sdk-host/tests/stdio_e2e.rs` — 保持 plain Client 回归。
- Modify: `echo-sdk-host/config.sdk.example.json` — 增加 facade 构建/limits 示例。
- Modify: `scripts/check-sdk-contracts.sh` — 纳入 catalog、RPC 和 handler coverage。
- Modify: `.github/workflows/rust-ci.yml` — 在 sdk-host 分组加入 facade E2E。
- Create: `docs/sdk/facade-feature-adapters.md` — 记录 canonical route、feature、resource、stream 和错误边界。
- Modify: `docs/sdk/README.md` — 导航 facade adapter，保持语言 SDK 未 Runnable。
- Modify: `docs/sdk/protocol.md` — 更新 Task/Subagent/facade 方法参考。
- Modify: `docs/sdk/sdk-core-profile.md` — 说明 core handle 与 facade resource 边界。
- Modify: `docs/sdk/sdk-extension-bridge.md` — 说明 facade route 与 consumer trait 边界。
- Modify: `docs/sdk/acp-standard-host.md` — 增加 minimal/full facade 构建说明。
- Modify: `docs/adr/0028-source-first-multilanguage-sdk-runtime.md` — 追加 canonical registry/feature/resource 决策。
- Modify: `README.md` — 更新 Rust Host facade adapter 状态。
- Modify: `README.zh.md` — 同步中文状态。
- Modify: `CHANGELOG.md` — 记录 facade adapter 语义。

## Reuse

- `echo-sdk-protocol/src/inventory.rs` — manifest_entries、classify_entry、adapter_for：保留 inventory/signature/feature 生成，替换 wildcard route。
- `echo-sdk-protocol/src/methods.rs` — FeatureOperationRequest、Task/Subagent DTO、WireValue：原位收敛 typed contract。
- `echo-sdk-protocol/src/catalog.rs` — METHOD_CATALOG：扩展 family binding，禁止额外 namespace。
- `echo-agent/src/acp/runtime.rs` — AcpConnectionServices、RunEntry：复用 connection/Session/Run/cancel/close authority。
- `echo-sdk-host/src/core_profile/mod.rs` — SdkCoreProfile::attach：复用一次官方 Builder composition。
- `echo-sdk-host/src/core_profile/state.rs` — CoreProfileState：扩展 advertisement、resource registry、settlement wait。
- `echo-sdk-host/src/core_profile/handles.rs` — HandleRegistry：复用 generation/kind/closed/owner/tombstone。
- `echo-sdk-host/src/factory.rs` — PreparedAgentDefinition::create_agent：在 concrete Agent 装箱前注入 sidecar。
- `echo-agent/src/agent/react/builder.rs` — ReactAgentBuilder::task_revision_service：复用 Task service 注入点。
- `echo-agent/src/agent/react/mod.rs` — ReactAgent::subagent_registry、subagent_executor：复用同一 Subagent authority。
- `echo-agent/echo-orchestration/src/tasks/revisioned.rs` — TaskRevisionService：唯一 Task CRUD/CAS。
- `echo-agent/echo-orchestration/src/tasks/runtime_service.rs` — RuntimeTaskService：唯一 DAG/retry/claim/terminal。
- `echo-agent/src/agent/subagent/executor.rs` — SubagentExecutor：唯一 dispatch/control/outcome。
- `echo-agent/echo-orchestration/src/workflow/mod.rs` 与 `echo-agent/src/workflow/` — Workflow、Graph、SharedState：直接调用既有 workflow。
- `echo-agent/src/state/mod.rs` 与 `echo-agent/echo-state/src/journal/` — RuntimeStateStore、EventJournal：复用 checkpoint/sequence/recovery。
- `echo-agent/echo-state/src/delivery.rs` — DeliveryLedger：复用 claim/settlement/recovery。
- `echo-agent/src/trace/mod.rs`、`echo-agent/src/eval/`、`echo-agent/src/improve/` — RunStore、TraceAnalyzer、EvalRunner、ImprovementLoop：复用观察/评测语义。
- `echo-agent/echo-integration/src/mcp/mod.rs` 与 `echo-agent/src/a2a/` — McpManager、A2AClient、A2AServer：只适配生命周期和结果。
- `.github/workflows/rust-ci.yml` — 现有 sdk-host/sdk-contract 分组：追加独立信号，保持低资源拆组。

## Todos

### freeze-executable-facade-catalog

requirements:
- § 7.1 权威集合
- § 7.2 Parity manifest
- § 10.4 echo-agent SDK extension families
- § 10.5 路径、数值与投影边界
- § 10.6 错误合同
- § 13 Feature 模型
- § 18 版本与兼容策略
- § 20.1 公共面完整性
- § 20.2 合同一致性

interfaces:
- consumes: root rustdoc inventory、signature digest、FeatureSemantics、WireValue、WireHandle、ExtensionKind、METHOD_CATALOG 和已交付 core/bridge route。
- produces: FacadeAdapterCatalog、canonical/alias route、FacadeResource/Stream DTO、required-feature/handler obligation、facade-operation-catalog.json 和生成合同。

steps:

1. 按 canonical source identity 审核 inventory，合并 re-export alias，分离 language intrinsic 与真正远程 operation。
   verify: 每个 facade item 只有一条 route，alias 不复制 handler，executable catalog 不含 wildcard。
   expected: parity manifest 逐项覆盖 root facade，但 Host 只看到 canonical obligations。

2. 定义 versioned catalog 与 typed DTO，固定 operation/signature、receiver/resource type、feature semantics、bounds、result/stream/error。
   verify: 正反 fixture 拒绝 unknown operation、错误 signature、extra field、unsafe integer、wrong handle/type/generation、missing feature 和 stream terminal 错误。
   expected: 语言 SDK 可直接消费生成合同，不需反射 Rust API。

3. 让 inventory/parity/schema/source-contract 从同一 catalog 生成 facade-operation-catalog.json，并检查 manifest、method catalog、handler obligation 一致。
   verify: 每个远程 route 有精确 handler 与真实验证引用，每个 handler 被至少一个 canonical item 使用。
   expected: 新增 public facade 或 route 漂移在合同生成阶段阻断。

### build-facade-runtime-and-feature-topology

requirements:
- § 6.1 框架层
- § 8 SDK 对象模型
- § 10.1 协议分层与通道纪律
- § 10.2 初始化与双 profile 协商
- § 11.2 Replay 与背压
- § 13 Feature 模型
- § 15 异常与边界场景
- § 16 安全与本地边界
- § 17 源码布局与交付合同
- § 20.4 可靠性

interfaces:
- consumes: FacadeAdapterCatalog、SdkCoreProfile/CoreProfileState/HandleRegistry、AcpConnectionServices、官方 Builder 和 Host limits。
- produces: sdk-facade-adapters/sdk-facade-all/leaf passthrough、compiled feature authority、FacadeAdapterRegistry、SessionFacadeRuntime、FacadeResource/Stream runtime。

steps:

1. 增加 facade 基础、all 与 root leaf passthrough features，用一份声明和 Cargo metadata 生成 advertisement。
   verify: root/Host feature 集合一致，default/core/bridge/facade-min/facade-all 和 all-of 组合只 advertise 实际能力。
   expected: Cargo 编译集合成为唯一能力权威，配置不再复制 feature bool。

2. 在 CoreProfileState/HandleRegistry 扩展静态 adapter/resource/stream registry，记录 canonical type、feature、owner、generation、bounds 和 close。
   verify: handle ladder、ABA、跨 Session、重复 close、drop、backpressure、timeout、cancel、shutdown 全部确定。
   expected: Host 只管理寻址和生命周期，Rust service 保持业务权威。

3. 将 facade 请求和 stream control 接入同一 attach builder，长操作 spawn 到官方 connection task。
   verify: plain/core/bridge-only 对 facade 保持 method-not-found；已协商缺 feature 返回 feature_unavailable；full handler coverage 无缺口。
   expected: 所有 profile 共用一个 transport、writer、Session/Run 和 close chain。

### wire-task-subagent-structured-authorities

requirements:
- § 4.1 已有通用权威
- § 5.1 范围
- § 10.4 echo-agent SDK extension families
- § 11.1 事件权威
- § 12.3 并发与死锁约束
- § 14.3 SDK 主动取消
- § 15. 异常与边界场景
- § 20.3 行为一致性
- § 20.4 可靠性

interfaces:
- consumes: SessionFacadeRuntime、TaskRevisionService、RuntimeTaskService、SubagentRegistry/SubagentExecutor/EventBus、Agent structured-output path 和专用 handles。
- produces: typed Task/Subagent/StructuredOutput handlers，全部绑定现有 Session/Run/event authority。

steps:

1. 将 Task create/update/list/execute/control 绑定 TaskRevisionService 和 RuntimeTaskService，create 原子返回 TaskRun/PlanTask handles，update 使用 revision/CAS。
   verify: RPC 与 Agent task tools 观察同一 revision、DAG、retry、pause/resume/cancel、interruption 和 terminal。
   expected: Host 不重算 frontier、claim、retry 或终态。

2. 在 concrete ReactAgent 装箱前登记同一 SubagentRegistry/Executor sidecar，dispatch/await/message/guidance/interrupt/cancel 复用真实 control identity。
   verify: sync/background/teammate、timeout、disconnect、late result、Session close 真实可观察，不能出现 pause/resume 伪语义或第二 executor。
   expected: RPC、Agent dispatch tool 和 Task execution共享一个 Subagent control plane。

3. structured output 复用 Agent/Run/provider capability、schema bound、typed parse error 和唯一 terminal。
   verify: valid、invalid schema、provider unavailable、payload bound、cancel/model failure 均保持 framework 结果。
   expected: 不旁路 AgentTurnDriver。

### implement-stateful-facade-families

requirements:
- § 4.1 已有通用权威
- § 5.1 范围
- § 10.4 echo-agent SDK extension families
- § 11.2 Replay 与背压
- § 11.3 Session 与进程恢复
- § 13 Feature 模型
- § 14.1 正常执行
- § 15 异常与边界场景
- § 20.3 行为一致性
- § 20.4 可靠性

interfaces:
- consumes: adapter/resource/stream registry、Store/Conversation/Compression、Workflow/Graph/SharedState、RuntimeStateStore/EventJournal、DeliveryLedger、RunStore/TraceAnalyzer、Eval/Improve。
- produces: memory/workflow/state/delivery/trace/eval/improve adapters 与其 resource/stream/error/recovery/close 行为。

steps:

1. 为 memory、conversation、compression 建 typed construction/query/mutation routes，调用现有 Store authority，显式传递路径和 namespace/search/pagination。
   verify: in-memory/file/sqlite、事务失败、reopen、compression input/output 和 missing feature 保持 Rust 语义。
   expected: Host 不把 EKO SQLite 规则带入通用 framework。

2. 为 Workflow/Graph/SharedState/checkpoint 建 resource/stream routes，直接调用既有 run/run_stream/checkpoint；闭包/callback 通过 ExtensionBridge。
   verify: sequential/DAG/parallel/conditional/checkpoint/interruption/stream cancel/close/callback failure 通过真实 Host。
   expected: Host 不复制 workflow graph 或 checkpoint 状态机。

3. 为 RuntimeStateStore/EventJournal/DeliveryLedger/RunStore/TraceAnalyzer/Eval/Improve 建 typed routes，保留原子写、sequence、claim/settlement、recovery 和 all-of feature。
   verify: concurrent CAS/append、replay/prune、delivery reopen、trace query、Eval report 和 improvement trajectory 的成功/冲突/失败可观察。
   expected: facade resource 只引用既有对象，Host 不成为数据或分析权威。

### complete-optional-feature-and-extension-routes

requirements:
- § 5.1 范围
- § 10.4 echo-agent SDK extension families
- § 12.1 统一模型
- § 12.2 Trait 映射
- § 12.3 并发与死锁约束
- § 13 Feature 模型
- § 15 异常与边界场景
- § 16 安全与本地边界
- § 20.1 公共面完整性
- § 20.3 行为一致性

interfaces:
- consumes: canonical catalog、Host feature passthrough、FacadeAdapterRegistry、已交付 ExtensionBridge 和 root facade leaf features。
- produces: MCP/A2A/LSP/channels/telemetry/topology、web/files/shell/git/database/rag/chart/media/data/statistics/research/content-guard/project-rules/testing routes，以及全部剩余 public extension 的闭集映射。

steps:

1. 适配 MCP、A2A、LSP、channels、telemetry、topology 的既有 client/server/manager lifecycle、request、stream、cancel、close。
   verify: loopback MCP/A2A、mock LSP/process、fake channel、isolated telemetry collector 有真实请求/事件/关闭；未编译组合返回 feature_unavailable。
   expected: 不重定义外部协议。

2. 适配 web/files/shell/git/database/rag/chart/media/data/statistics/research/content-guard/project-rules/testing 的 canonical operation，继续遵守现有 permission、sandbox、cwd、timeout、cancel。
   verify: local fixture、tempdir、git、sqlite、mock Embedder/HTTP 覆盖结果、副作用、错误、取消、关闭。
   expected: 不复制算法或绕过 framework policy。

3. 审核全部 Extension classification，为剩余 consumer trait 增加 typed ExtensionBridge kind/proxy 或重分类 language intrinsic，处理 stream/exclusive/cancel/disconnect/close。
   verify: extension obligation 与 bridge handler/proxy 集合相等，每个远程 trait 有成功、failure、timeout、cancel、disconnect、late-response 证据。
   expected: 不存在 manifest extension 但 Host 不可注册的假对等。

### prove-full-facade-and-document-status

requirements:
- § 17 源码布局与交付合同
- § 18 版本兼容策略
- § 19 文档与示例
- § 20.1 公共面完整性
- § 20.2 合同一致性
- § 20.3 行为一致性
- § 20.4 可靠性
- § 20.5 源码交付
- § 20.6 状态声明

interfaces:
- consumes: catalog、compiled handler/feature set、family adapters、真实 Host E2E 和 standard/core/bridge regressions。
- produces: facade adapter evidence、generated contracts/CI/docs/ADR 和诚实 Rust Host facade-adapter status。

steps:

1. 用官方 ACP Client 运行 full-facade Host family matrix，覆盖 Task/Subagent/structured output、stateful families、optional integrations/tools、resources/streams、cancel/disconnect/late response/teardown。
   verify: 每个 canonical handler family 有真实成功和错误/feature/handle/close 证据，handler coverage 零缺口。
   expected: E2E 成为后续三语言 SDK 的行为基线。

2. 对 standard、core-only、bridge-only、facade-min、facade-all 和每个 root leaf passthrough 执行隔离构建/行为检查，重跑合同生成和仓库完整门禁。
   verify: minimal 不误广告，单 feature 无隐式依赖，all-of 准确，Schema/fixtures/source-contract/parity/catalog 无漂移，CI 分组未被削弱。
   expected: 所有适用门禁退出 0。

3. 更新 docs/sdk 唯一入口、protocol/core/bridge/standard-host、ADR、README、CHANGELOG、CI，检查 existing Rust examples 并将 Host E2E 作为本阶段可执行示例。
   verify: 文档 method/capability/feature/resource/stream/error/build/status 与合同和测试一致；链接可解析；语言 SDK 仍 not_implemented。
   expected: 只声明 Rust Host facade adapter available，SDK-Skill-Impact 为 none。
