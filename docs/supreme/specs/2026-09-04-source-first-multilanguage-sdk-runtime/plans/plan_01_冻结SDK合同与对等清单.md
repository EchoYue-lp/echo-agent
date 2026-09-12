---
schema_version: 3
supersedes: null
slug: 2026-09-04-source-first-multilanguage-sdk-runtime/plan
goal: 冻结 echo_agent 根 facade 的完整跨语言对等基线和可生成协议合同，使后续 Host 与三语言 SDK 只能消费一套可检查的公共面与
  wire 语义。
ships: 一套覆盖根 echo_agent 全 feature 公共面的确定性 inventory、逐项三语言 parity manifest、版本化
  JSON-RPC wire DTO/Schema 与 golden fixtures，以及阻断漂移的合同检查；不实现 Host 或语言 SDK。
verify: 从干净 checkout 重新生成全 feature facade inventory、parity manifest、JSON Schema
  和 golden fixtures 后零差异，遗漏公共项或非法协议 fixture 会稳定失败；cargo fmt
  --all、./scripts/verify.sh 及 AGENTS.md 规定的逐 feature 条件矩阵全部通过。
design_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md
delivery_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/plans/delivery-map.md#sdk-contract-inventory
todos:
  - id: freeze-facade-inventory
    files:
      - Cargo.toml
      - Cargo.lock
      - echo-sdk-protocol/Cargo.toml
      - echo-sdk-protocol/src/lib.rs
      - contracts/sdk/toolchain.json
      - contracts/sdk/public-api.txt
      - contracts/sdk/parity-manifest.schema.json
      - contracts/sdk/parity-manifest.json
      - echo-sdk-protocol/tests/facade_inventory.rs
    summary: 用固定 rustdoc/public-api 工具链冻结根 facade 的全 feature inventory，并建立逐项三语言
      parity manifest。
    verify: 默认、full 和每个叶 feature 的公开项都进入确定性 inventory；manifest 对每项给出唯一分类、feature 条件及
      TS/Python/Java 映射状态，新增、缺失和重复项均 fail closed。
  - id: define-sdk-wire-contract
    files:
      - Cargo.toml
      - Cargo.lock
      - echo-sdk-protocol/Cargo.toml
      - echo-sdk-protocol/src/lib.rs
      - echo-sdk-protocol/src/rpc.rs
      - echo-sdk-protocol/src/initialize.rs
      - echo-sdk-protocol/src/error.rs
      - echo-sdk-protocol/src/scalar.rs
      - echo-sdk-protocol/src/handle.rs
      - echo-sdk-protocol/src/catalog.rs
      - contracts/sdk/parity-manifest.json
    summary: 定义独立于进程内 Rust 类型的严格 JSON-RPC、初始化、错误、标量、handle 和 operation catalog 合同。
    verify: 协议拒绝 batch、空或数字 ID、result/error 同存和无损边界违规；初始化、feature、limits、handle
      generation、stream 与 extension 语义能覆盖 manifest 分类且不复制框架状态机。
  - id: generate-schema-fixtures
    files:
      - echo-sdk-protocol/src/schema.rs
      - echo-sdk-protocol/src/bin/export_schema.rs
      - echo-sdk-protocol/tests/protocol_contract.rs
      - contracts/sdk/schema/host-protocol-v1.schema.json
      - contracts/sdk/fixtures/protocol/v1
    summary: 从 Rust wire DTO 生成确定性 Schema、合同 digest 和覆盖正常与失败边界的 golden fixtures。
    verify: Schema、digest 和 fixtures 可重复生成且工作树零差异；Rust合同测试接受所有有效
      fixture，并拒绝版本、ID、整数、路径、载荷、unknown 和错误闭集的非法样本。
  - id: gate-and-document-contract
    files:
      - scripts/check-sdk-contracts.sh
      - scripts/verify.sh
      - .github/workflows/rust-ci.yml
      - docs/sdk/README.md
      - docs/sdk/protocol.md
      - README.md
    summary: 把 SDK合同漂移检查接入现有门禁，并建立只声明 Contract 状态的唯一公共文档入口。
    verify: 本地与 CI 都执行确定性 contract check；docs/sdk 清楚说明源码交付、工具链、公共面、非目标和状态，根 README
      只导航到该入口且不宣称 Host、语言 SDK 或发布产物已经存在。
lifecycle: superseded
artifact_id: plan:ce5020a2-364f-4ce9-a23f-a0c2ff182afa
design_revision: sha256:f494aad992841882ad561c428ec03f7652c55966ad67017981e46992233b4b3d
---
## Approach

- 采用 contract-first 切片：先冻结根 facade 公共面、三语言对等分类和协议 DTO，再允许后续 outcome 实现 Host。当前结果完成后应达到 Contract，但仍不是 Runnable 或 Parity complete。
- 公共 inventory 以根 package echo_agent 的 rustdoc 可达项为权威，覆盖 default、full 和每个叶 feature 的并集与差量；不得用 prelude、README feature 表或手写模块清单代替。
- 使用固定且仓库可复现的 rustdoc/public-api 工具链生成 inventory。生成和检查必须是结构化流程，不用正则扫描 Rust 源码猜测 re-export。
- 为 wire 新建 protocol-owned DTO 和无损 adapter obligation；不为方便生成而给 TurnRequest、TurnReceipt、ChatRequest、ToolContext、AgentHandle 等进程内类型批量增加 serde 或直接暴露内部所有权。
- parity manifest 同时约束 wire_value、operation、handle、stream、extension、language_intrinsic 六类映射。当前没有三语言实现时如实记录未完成状态，不能把生成类型写成对等完成。
- 合同检查复用现有本地门禁和分组 CI，不恢复单一高内存 workspace test job。

## Global Constraints

- SDK 对等权威仅是根 echo_agent facade 正式公开项及全部公开 feature；workspace 子 crate 的内部 pub 项不进入合同。
- Rust 继续是唯一 Agent、Run、Task、Subagent、重试、取消和恢复语义权威；本计划不得实现 Host dispatcher、Session/Run lifecycle 或语言 SDK。
- 只使用 Subagent 术语；不得新增 Worker/worker 命名。
- 所有 Rust 代码遵守仓库 UTF-8 安全和无 panic 约束，不使用 unwrap、expect、panic、unreachable 或可能越界的直接索引。
- JSON-RPC framing 使用 UTF-8 单行 compact JSON；request ID 为非空字符串，response 必须 result/error 二选一，batch 不在合同范围。
- u64、usize、sequence、revision 等超过 JavaScript 安全整数范围的值必须使用无损 wire 表达；不得依赖 JSON number 舍入。
- Path、Duration、binary、timestamp、unknown enum/event 和泛型 payload 必须有跨平台、可往返的明确表示；显示文本不能替代原始路径或二进制身份。
- feature inventory 必须覆盖 data 及 Cargo.toml 中全部叶 feature。当前 docs.rs 排除 data 的 nightly/Polars 限制不能被静默继承为 SDK 缺口。
- echo-sdk-protocol 必须设置 publish = false，并进入 workspace members/default-members；本计划不新增任何 registry 发布配置。
- 普通消费者构建不得自动安装工具链或下载本项目预编译产物。贡献者合同检查可使用声明并锁定的第三方源码依赖和专用 rustdoc 工具链；不在线下载仅指不下载本项目发布的 Host/SDK 产物，不要求 vendor 所有生态依赖。
- Schema、fixture、inventory 和 parity manifest 的更新只能由显式生成模式完成；默认检查模式只读并在漂移时失败。
- 当前状态只能从 Design 提升为 Contract；文档不得宣称 Host 可运行、三语言已对等或 npm/PyPI/Maven/二进制已发布。
- 不修改 echo-agent-cli 或 echo-website；网站入口属于 delivery map 中依赖 Parity complete 的独立结果。

## Files

- Modify: `Cargo.toml` — 注册 protocol workspace member，并加入合同生成/验证所需的最小依赖或配置。
- Modify: `Cargo.lock` — 锁定新增 Rust 合同工具依赖。
- Create: `contracts/sdk/toolchain.json` — 固定 public-api/rustdoc 合同工具链及兼容信息。
- Create: `contracts/sdk/public-api.txt` — 根 facade default/full/逐叶 feature 的确定性公共面快照。
- Create: `contracts/sdk/parity-manifest.schema.json` — 约束公共项 identity、feature、分类和三语言映射状态。
- Create: `contracts/sdk/parity-manifest.json` — 覆盖全部 facade 项的对等清单和 protocol adapter obligation。
- Create: `echo-sdk-protocol/Cargo.toml` — product-neutral、source-only 的 wire contract crate。
- Create: `echo-sdk-protocol/src/lib.rs` — 导出唯一协议公共面。
- Create: `echo-sdk-protocol/src/rpc.rs` — 严格 JSON-RPC request/response/notification 合同。
- Create: `echo-sdk-protocol/src/initialize.rs` — protocol version、source digest、feature/capability/limits 协商。
- Create: `echo-sdk-protocol/src/error.rs` — 稳定错误 code、retryability、identity 与有界 details。
- Create: `echo-sdk-protocol/src/scalar.rs` — 无损整数、duration、path、binary、timestamp 与 unknown 表达。
- Create: `echo-sdk-protocol/src/handle.rs` — 不透明 object identity 与 generation fence wire 值。
- Create: `echo-sdk-protocol/src/catalog.rs` — 从 parity manifest 对齐的 canonical operation/extension catalog。
- Create: `echo-sdk-protocol/src/schema.rs` — Schema、fixture 与 contract digest 的确定性生成入口。
- Create: `echo-sdk-protocol/src/bin/export_schema.rs` — 显式生成/检查命令入口。
- Create: `echo-sdk-protocol/tests/facade_inventory.rs` — root facade inventory 与 manifest coverage 合同。
- Create: `echo-sdk-protocol/tests/protocol_contract.rs` — wire、Schema 和 fixture 正反例合同。
- Create: `contracts/sdk/schema/host-protocol-v1.schema.json` — 从 Rust DTO 生成的版本化协议 Schema。
- Create: `contracts/sdk/fixtures/protocol/v1` — 跨语言复用的有效与无效 golden fixtures。
- Create: `scripts/check-sdk-contracts.sh` — 默认只读的 inventory/manifest/schema/fixture 漂移检查。
- Modify: `scripts/verify.sh` — 将 SDK contract check 纳入本地完整门禁，不改变原有 Rust检查。
- Modify: `.github/workflows/rust-ci.yml` — 在现有资源分组内增加独立合同信号，不恢复高内存单一测试 job。
- Create: `docs/sdk/README.md` — 唯一 SDK外部文档入口，准确声明 Contract/source-only 状态。
- Create: `docs/sdk/protocol.md` — 公共面、wire规则、版本和错误合同说明。
- Modify: `README.md` — 导航到唯一 SDK文档入口并保持当前状态准确。

## Reuse

- `src/lib.rs:31` — 根 facade 模块与 re-export — public inventory 必须从这里可达的 rustdoc 公共面解析，而非只扫描 prelude。
- `Cargo.toml:55` — docs.rs/full/叶 feature 定义及 data 限制 — 生成实际 feature profile 并显式验证 data。
- `tests/facade_smoke.rs:1` — 当前 facade compile smoke — 保留为人工重点能力 smoke，新增 inventory 合同不复制这些断言。
- `echo-core/src/agent/event_envelope.rs:12` — EventEnvelope version/identity/sequence/hash — protocol event adapter 必须保留这些既有事实。
- `echo-orchestration/src/runtime/turn_driver.rs:103` — TurnRequest 与进程内取消/上下文 — 记录 adapter obligation，不直接作为 wire DTO。
- `echo-orchestration/src/runtime/turn_driver.rs:418` — TurnReceipt 与唯一终态结果 — 记录无损 receipt DTO 义务，不在当前 outcome 驱动执行。
- `echo-core/src/tools/mod.rs:323` — ToolResult 的非 wire model_content — protocol mapping 必须显式覆盖，不能依赖现有 serde 跳过字段。
- `src/agent/handle.rs:337` — 进程内 AgentHandle — wire handle 只复用对象语义，不暴露 Arc/RwLock/closure escape hatch。
- `echo-agent-learning/tests/documentation_contract.rs:115` — 清单唯一与 fail-closed 文档/示例合同模式 — 复用其确定性 inventory/check 思路。
- `scripts/verify.sh:1` — 当前本地完整 Rust门禁入口 — 增量接入合同检查。
- `.github/workflows/rust-ci.yml:22` — Linux quality 与分组测试结构 — 保持资源边界并增加独立合同信号。

## Todos

### freeze-facade-inventory

requirements:
- § 1. 问题与目标
- § 2. 已确认决策
- § 5. 范围与非目标
- § 7. 公共 API 权威与对等清单
- § 13. Feature 模型
- § 17. 源码布局与交付合同
- § 20. 验收标准
- § 21. 关键取舍与后果
- § 22. 当前状态

interfaces:
- consumes: 根 echo_agent rustdoc 可达公共面、Cargo feature graph、现有 facade smoke 和固定 public-api/rustdoc 工具链；同时创建 protocol crate 的最小 workspace 基础。
- produces: echo-sdk-protocol 的最小 crate 基础、contracts/sdk/public-api.txt、parity-manifest.schema.json、parity-manifest.json，以及 default/full/逐叶 feature 的确定性 inventory check。

steps:

1. 固定 inventory 工具与专用 rustdoc toolchain，按 Cargo metadata 展开 full 和所有叶 feature，并生成根 echo_agent 的结构化公共项集合及确定性文本快照。
   verify: 重复生成得到相同顺序与内容；glob re-export、impl method、宏和 data feature 不被静默遗漏。
   expected: public-api 快照可以审阅根 facade 的真实边界，且不依赖 prelude 或 README 手写列表。
2. 定义 parity manifest 的机器 schema，并把每个公共项唯一归入 wire_value、operation、handle、stream、extension 或 language_intrinsic，记录 feature 条件和三语言目标映射/当前状态。
   verify: 每个 inventory identity 恰有一个 manifest entry，重复、未知分类、非法 feature 和缺少任一语言字段均返回非零。
   expected: 后续协议、Host 与语言计划能按稳定 identity 消费同一清单，未实现项不会被误报为完成。
3. 建立双向 drift check：inventory 新增/删除/签名变化要求显式更新 manifest，manifest 中不存在的公共项或已消失 identity 都阻断验证。
   verify: 注入一个未映射公共项、重复 identity 或 stale manifest entry 时检查稳定失败并给出具体 rust path/feature。
   expected: facade 演进无法绕过三语言 SDK影响评估。

### define-sdk-wire-contract

requirements:
- § 4. 当前代码事实与复用结论
- § 5. 范围与非目标
- § 6. 系统边界与分层
- § 7. 公共 API 权威与对等清单
- § 8. SDK 对象模型
- § 9. 语言惯用 API
- § 10. 协议合同
- § 12. Extension Bridge
- § 13. Feature 模型
- § 15. 异常与边界场景
- § 16. 安全与本地边界
- § 21. 关键取舍与后果

interfaces:
- consumes: 上一 Todo 交付的 echo-sdk-protocol crate 基础、public-api inventory、parity manifest 分类、EventEnvelope identity/sequence、现有 facade 值和 trait 语义。
- produces: echo_sdk_protocol RpcMessage/RpcRequest/RpcResponse/RpcNotification/InitializeRequest/InitializeResponse/SdkError/WireHandle/WireU64/WireDuration/WirePath/WireBytes 与 canonical operation/extension catalog。

steps:

1. 定义严格 JSON-RPC 2.0 envelope 和反序列化校验，分别表达 request、response、notification 与 error，禁止 batch、空/数字 ID、result/error 同存及未知顶层形状。
   verify: 每个合法消息恰好匹配一个 variant；歧义或非法消息返回稳定 protocol error，不 panic、不降级为任意 JSON。
   expected: 后续 Host 与三语言 dispatcher 共用一套可判别、可演进的 framing 合同。
2. 定义无损 scalar/value 规则：大整数使用十进制 wire 值，Duration 保留秒/纳秒，Path 保留平台原始编码和可选显示值，binary 使用明确编码，timestamp 与 unknown 值可往返。
   verify: u64 最大值、usize 边界、非 UTF-8 Unix path、Windows UTF-16 path、空/大 binary、纳秒 Duration 和 unknown event/type 均有确定序列化结果与非法输入错误。
   expected: TS 不受 53-bit number 限制，三语言不会用显示字符串替代原始值。
3. 定义 initialize、source/contract digest、protocol range、required/optional capability、实际叶 feature、limits、handle identity/generation、stream 和 extension 基础 DTO。
   verify: 无共同 protocol、digest不兼容、缺必需 capability/feature、非法 generation 与超限声明均 fail closed，且初始化失败前不产生对象 handle。
   expected: 后续 Host 可以在执行前完成完整能力协商，SDK 不需要猜测或 fallback。
4. 从 parity manifest 形成 canonical operation/extension catalog，并为非 wire-ready Rust 类型记录独立 DTO/adapter obligation，不给进程内类型批量添加 serde。
   verify: TurnRequest/TurnReceipt、ChatRequest/Response/Chunk、ToolResult/ToolContext、AgentHandle、closure DSL 和 generic facade 都有明确 mapping，不丢字段、不暴露锁或内存地址。
   expected: Host 实现只能实现已冻结 operation，并通过薄 adapter 调用现有 framework authority。

### generate-schema-fixtures

requirements:
- § 10. 协议合同
- § 11. 事件、结果与恢复
- § 12. Extension Bridge
- § 15. 异常与边界场景
- § 18. 版本与兼容策略
- § 20. 验收标准

interfaces:
- consumes: echo_sdk_protocol wire DTO、canonical catalog、parity manifest 和固定生成工具链。
- produces: host-protocol-v1.schema.json、contract/source digest、版本化 valid/invalid golden fixtures 和默认只读的 deterministic generation check。

steps:

1. 从 Rust wire DTO 和 catalog 生成单一版本化 JSON Schema，使用 canonical serialization 计算可复现 contract digest。
   verify: 同一源码 revision 在重复运行和干净 checkout 中生成相同 Schema、排序和 digest。
   expected: Host 与语言 SDK 可以用 digest 检测合同漂移，而不是只比较 crate version 0.2.0。
2. 建立覆盖 initialize、RPC envelope、错误、scalar、handle、stream、extension、unknown 与 bounds 的有效和无效 fixtures。
   verify: 所有有效 fixture 可往返且保持 identity/值；所有无效 fixture 被指定 error code 拒绝。
   expected: 后续 TS/Python/Java 复用同一输入，不各自解释协议。
3. 提供显式 update 模式和默认 check 模式；check 模式重新生成到隔离位置并与提交产物比较。
   verify: 默认模式不修改仓库；显式 update 后再次 check 零 diff，手工编辑生成文件会稳定失败。
   expected: 生成产物可审阅且不会因普通测试静默重写。

### gate-and-document-contract

requirements:
- § 2. 已确认决策
- § 3. 业界参考与取舍
- § 5. 范围与非目标
- § 17. 源码布局与交付合同
- § 18. 版本与兼容策略
- § 19. 文档与示例
- § 20. 验收标准
- § 22. 当前状态

interfaces:
- consumes: inventory/parity/schema/fixture 检查入口、现有 scripts/verify.sh 与分组 Rust CI。
- produces: scripts/check-sdk-contracts.sh、接入后的本地/CI合同门禁，以及 docs/sdk 唯一入口和 Contract 状态说明。

steps:

1. 增加统一合同检查入口，并接入 scripts/verify.sh 和现有 Linux 分组 CI；固定工具缺失时给出明确前置条件，不在普通检查中隐式安装。
   verify: 本地与 CI 对 inventory、manifest、Schema、fixtures 的相同漂移给出相同非零结果；原 Rust门禁和低资源分组不被删除。
   expected: 公共 facade 或协议变化无法在只通过编译时绕过 SDK合同检查。
2. 创建 docs/sdk/README.md 与 protocol.md，说明源码交付、工具链、根 facade 边界、语言对等分类、版本/错误规则和当前仅 Contract 状态，并从根 README 导航。
   verify: 文档不声称 Host、TS/Python/Java、registry package 或预编译二进制已经可用，所有本地链接可解析。
   expected: 外部读者能区分 Design、Contract、Runnable、Parity complete 与 Published。
3. 执行仓库既有完整门禁和适用的逐 feature 条件矩阵，确认 contract tooling 不破坏默认、all-feature、featureless 或单 feature 构建。
   verify: cargo fmt --all、./scripts/verify.sh 和 AGENTS.md 条件矩阵全部退出 0；生成检查后工作树无额外 diff。
   expected: 当前 outcome 可独立合并并停止，后续 Host plan 只消费已验证合同。

## Decisions

- 当前 Plan 只交付 Contract，不实现 echo-agent-sdk-host、Agent/Session/Run、extension dispatcher 或任何语言 SDK。
- public inventory 覆盖根 facade 的 rustdoc 可达项，不把 prelude 当完整公共面，也不把 workspace 子 crate 内部 pub 项纳入。
- protocol-owned DTO 与 adapter obligation 是唯一 wire 入口；禁止为减少工作量批量序列化进程内类型。
- contract digest 与 Git/source identity 共同约束源码版本，不能只比较 0.2.0 package version。
- cargo-public-api/rustdoc 工具链必须显式固定并兼容 data feature；无法生成 data inventory 时当前 outcome 不得完成。
- 第三方源码依赖可以按 manifest/lockfile解析；禁止的是下载本项目预编译 Host/SDK产物，不要求仓库 vendor 全部生态依赖。
- examples 在本 outcome 无可运行 SDK API，明确保持不变；echo-website 必须等待 Parity complete 后由独立 outcome 更新。
