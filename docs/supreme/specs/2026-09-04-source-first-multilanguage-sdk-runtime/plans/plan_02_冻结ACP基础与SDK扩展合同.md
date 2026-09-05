---
schema_version: 3
supersedes: plan:ce5020a2-364f-4ce9-a23f-a0c2ff182afa
slug: 2026-09-04-source-first-multilanguage-sdk-runtime/plan
goal: 冻结官方稳定 ACP v1 基线、echo_agent 根 facade 的 ACP relationship 对等清单和
  _echo_agent/* 扩展合同，使后续 Adapter、Host 与三语言 SDK 只消费一套标准基础和可检查扩展。
ships: 一套覆盖根 echo_agent 全 feature 公共面的确定性 inventory、ACP relationship parity
  manifest、锁定官方稳定 ACP v1 基线的 _echo_agent/* 扩展 Schema/fixtures 与阻断漂移检查；不实现 ACP
  Agent adapter、Host 或语言 SDK。
verify: 从干净 checkout 重新生成全 feature facade inventory、ACP baseline、parity
  manifest、echo-agent扩展Schema和fixtures后零差异，任何官方ACP分叉、未映射公共项或非法扩展样本稳定失败；cargo fmt
  --all、./scripts/verify.sh及AGENTS.md逐feature条件矩阵全部通过。
design_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md
delivery_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/plans/delivery-map.md#acp-sdk-contract
todos:
  - id: freeze-facade-acp-inventory
    files:
      - Cargo.toml
      - Cargo.lock
      - echo-sdk-protocol/Cargo.toml
      - echo-sdk-protocol/src/lib.rs
      - echo-sdk-protocol/tests/facade_inventory.rs
      - echo-sdk-protocol/tests/acp_baseline.rs
      - contracts/sdk/toolchain.json
      - contracts/sdk/acp-baseline.json
      - contracts/sdk/public-api.txt
      - contracts/sdk/parity-manifest.schema.json
      - contracts/sdk/parity-manifest.json
    summary: 固定官方稳定 ACP v1 artifact基线与根facade全feature inventory，并建立逐项ACP
      relationship和三语言parity manifest。
    verify: 默认、full和每个现有叶feature公开项都进入确定性inventory；ACP wire、crate/schema
      artifact版本独立锁定且官方schema未被复制；每项有唯一语义分类、ACP relationship和三语言状态。
  - id: define-echo-extension-contract
    files:
      - echo-sdk-protocol/src/capability.rs
      - echo-sdk-protocol/src/methods.rs
      - echo-sdk-protocol/src/error.rs
      - echo-sdk-protocol/src/scalar.rs
      - echo-sdk-protocol/src/handle.rs
      - echo-sdk-protocol/src/event.rs
      - echo-sdk-protocol/src/catalog.rs
      - contracts/sdk/parity-manifest.json
    summary: 只定义ACP _meta中的echo-agent capability与_echo_agent/*完整facade扩展，不重写标准ACP协议。
    verify: 所有自定义方法已协商且以下划线开头；ACP request
      ID遵循官方类型，扩展领域ID和无损值独立；标准ACP方法、envelope和Session/Prompt类型没有本地副本。
  - id: generate-extension-schema-fixtures
    files:
      - echo-sdk-protocol/src/schema.rs
      - echo-sdk-protocol/src/bin/export_schema.rs
      - echo-sdk-protocol/tests/extension_contract.rs
      - contracts/sdk/schema/echo-agent-extension-v1.schema.json
      - contracts/sdk/fixtures/extension/v1
    summary: 生成确定性的echo-agent扩展Schema、contract digest与标准/扩展边界golden fixtures。
    verify: 扩展Schema、digest和fixtures可重复生成且工作树零差异；有效样本无损往返，非法namespace、method、ID、整数、路径、载荷和unknown边界稳定失败，官方ACP
      fixtures未被复制。
  - id: gate-and-document-acp-contract
    files:
      - scripts/check-sdk-contracts.sh
      - scripts/verify.sh
      - .github/workflows/rust-ci.yml
      - docs/sdk/README.md
      - docs/sdk/protocol.md
      - README.md
    summary: 把ACP+SDK合同漂移检查接入现有门禁，并建立只声明Contract状态的唯一公共文档入口。
    verify: 本地与CI都执行inventory、ACP
      baseline、manifest及扩展Schema/fixture检查；docs/sdk准确说明双profile、源码交付和状态，根README不宣称Adapter、Host、语言SDK或发布产物已存在。
artifact_id: plan:b76eb4e0-64ec-4adb-ac12-8dab069a502c
lifecycle: ready
design_revision: sha256:9685cabd8f692ea060b67e46fe1c7d47531edb44efd998913e252803ef122817
---
## Approach

- 采用ACP-first contract切片：官方稳定ACP v1是唯一基础Client-Agent协议；当前Plan只固定官方artifact基线、根facade关系清单和echo-agent扩展，不创建私有JSON-RPC基础协议。
- 公共inventory以根package echo_agent的rustdoc可达项为权威，覆盖default、full和每个叶feature的并集与差量；不得用prelude、README feature表或手写模块清单代替。
- echo-sdk-protocol只定义ACP _meta中的echo-agent capability和_echo_agent/*扩展DTO；标准ACP envelope、initialize、Session、Prompt、ContentBlock、update、stop reason和request ID全部直接使用官方schema。
- parity manifest同时记录wire_value/operation/handle/stream/extension/language_intrinsic语义分类，以及standard/standard_projection/echo_extension/language_intrinsic ACP relationship。
- 生成物只包含echo-agent扩展Schema和fixtures；官方ACP schema与conformance fixtures通过精确artifact引用复用，不复制、不修改、不重新生成。
- 当前结果完成后只达到Contract，不是ACP conformant、Runnable或Parity complete。

## Global Constraints

- SDK对等权威仅是根echo_agent facade正式公开项及全部公开feature；workspace子crate内部pub项不进入合同。
- 稳定ACP wire protocolVersion固定为1；官方Rust/schema artifact版本必须使用实施时重新核验过的精确版本并写入Cargo.lock和acp-baseline.json，禁止latest、范围漂移和任何draft/unstable ACP feature。
- 不新增自有RpcMessage、RpcRequest、RpcResponse、RpcNotification、InitializeRequest、基础Session/Prompt DTO或JSON-RPC parser。
- ACP request ID完全遵守官方schema；Agent、Session、Run、Event、operation、extension、generation与幂等identity使用扩展payload中的独立非空字符串。
- 自定义数据只进入namespaced _meta echo-agent capability；自定义request/notification全部使用_echo_agent/*前缀，不能增加或改变ACP标准字段。
- 标准ACP路径保持绝对UTF-8字符串语义；非UTF-8 Unix path、Windows UTF-16、u64/usize、Duration、binary和完整EventEnvelope等无损表达只属于echo-agent扩展。
- 标准ACP投影允许受标准schema限制，但每个丢失的framework identity、状态、错误、artifact、cursor或recovery事实必须有echo_extension映射。
- Rust继续是唯一Agent、Run、Task、Subagent、重试、取消和恢复权威；本Plan不得新增根acp feature、ACP Agent adapter、Host、Session/Run service、extension dispatcher或语言SDK。
- 只使用Subagent术语，不新增Worker/worker命名。
- 所有Rust代码遵守UTF-8安全和无panic约束，不使用unwrap、expect、panic、unreachable或可能越界的直接索引。
- feature inventory必须覆盖data及Cargo.toml中的全部现有叶feature；docs.rs排除data的nightly/Polars限制不能成为SDK inventory缺口。
- echo-sdk-protocol设置publish = false并进入workspace members/default-members；不新增registry发布配置。
- 普通消费者构建不得自动安装工具链或下载本项目预编译产物；贡献者合同检查可以使用精确锁定的第三方源码依赖和专用rustdoc工具链。
- Schema、fixture、inventory和parity manifest只能由显式update模式更新；默认check模式只读并在漂移时失败。
- 不修改echo-agent-cli、echo-website或并无关的admission/ADR 0029改动；网站入口等待Parity complete。
- Plan 01只作为旧设计revision的历史记录，不能再作为执行来源。

## Files

- Modify: `Cargo.toml` — 注册非发布的echo-sdk-protocol workspace member，不新增根acp feature。
- Modify: `Cargo.lock` — 锁定合同工具与官方ACP schema artifact依赖。
- Create: `echo-sdk-protocol/Cargo.toml` — 声明source-only扩展合同crate及精确官方ACP schema依赖。
- Create: `echo-sdk-protocol/src/lib.rs` — 导出唯一echo-agent扩展合同面，不重新导出自建ACP envelope。
- Create: `echo-sdk-protocol/src/capability.rs` — 定义ACP _meta中的echo-agent capability、扩展版本、digest、feature和limits。
- Create: `echo-sdk-protocol/src/methods.rs` — 定义_echo_agent/* request/notification payload与方法catalog。
- Create: `echo-sdk-protocol/src/error.rs` — 定义扩展错误code、retryability、领域identity和有界details。
- Create: `echo-sdk-protocol/src/scalar.rs` — 定义仅供扩展使用的无损整数、Duration、Path、binary、timestamp和unknown值。
- Create: `echo-sdk-protocol/src/handle.rs` — 定义扩展对象identity和generation fence。
- Create: `echo-sdk-protocol/src/event.rs` — 定义完整EventEnvelope、replay、gap和snapshot扩展视图。
- Create: `echo-sdk-protocol/src/catalog.rs` — 绑定parity manifest的canonical扩展operation catalog。
- Create: `echo-sdk-protocol/src/schema.rs` — 生成扩展Schema、fixtures和contract digest。
- Create: `echo-sdk-protocol/src/bin/export_schema.rs` — 提供显式update和默认check入口。
- Create: `echo-sdk-protocol/tests/facade_inventory.rs` — 验证根facade inventory与parity manifest覆盖。
- Create: `echo-sdk-protocol/tests/acp_baseline.rs` — 验证精确官方ACP v1 artifact与本地不分叉边界。
- Create: `echo-sdk-protocol/tests/extension_contract.rs` — 验证扩展capability、方法、错误、标量、handle和event合同。
- Create: `contracts/sdk/toolchain.json` — 固定public-api/rustdoc合同工具链。
- Create: `contracts/sdk/acp-baseline.json` — 分别记录ACP wire、Rust crate和schema artifact版本及稳定feature集合。
- Create: `contracts/sdk/public-api.txt` — 根facade default/full/逐叶feature的确定性公共面快照。
- Create: `contracts/sdk/parity-manifest.schema.json` — 约束公共项identity、feature、语义分类、ACP relationship和三语言状态。
- Create: `contracts/sdk/parity-manifest.json` — 覆盖全部facade项的ACP/SDK映射与adapter obligation。
- Create: `contracts/sdk/schema/echo-agent-extension-v1.schema.json` — 从Rust扩展DTO生成的版本化Schema。
- Create: `contracts/sdk/fixtures/extension/v1` — namespaced扩展与标准/扩展边界的golden fixtures。
- Create: `scripts/check-sdk-contracts.sh` — 默认只读的inventory/ACP baseline/manifest/schema/fixture漂移检查。
- Modify: `scripts/verify.sh` — 将SDK合同检查纳入本地门禁，不改变原有Rust检查。
- Modify: `.github/workflows/rust-ci.yml` — 在现有资源分组内增加独立合同信号，不恢复高内存单一test job。
- Create: `docs/sdk/README.md` — 唯一SDK外部入口，准确声明ACP-first、source-only和Contract状态。
- Create: `docs/sdk/protocol.md` — 说明ACP标准profile、echo-agent扩展profile、版本、投影和错误边界。
- Modify: `README.md` — 导航到唯一SDK文档入口，不宣称ACP adapter、Host或语言SDK已经可用。

## Reuse

- `src/lib.rs:31` — 根facade模块与re-export — inventory从完整rustdoc可达面生成，不只扫描prelude。
- `Cargo.toml:55` — docs.rs/full/叶feature定义与data限制 — 展开真实feature profile并验证全部现有叶feature。
- `src/lib.rs:103` — A2A/MCP等可选协议module形态 — 只作为后续acp feature布局证据，当前Plan不新增module。
- `src/mcp.rs:1` — 薄协议facade模式 — 后续ACP adapter复用分层原则，但MCP transport和DTO不复用。
- `tests/facade_smoke.rs:1` — 当前facade compile smoke — 保留重点能力smoke，新增inventory合同不复制其断言。
- `echo-core/src/agent/event_envelope.rs:12` — EventEnvelope版本、identity、sequence和hash — 扩展event DTO必须无损保留这些事实。
- `echo-orchestration/src/runtime/turn_driver.rs:103` — TurnRequest及进程内取消/上下文 — 记录adapter obligation，不直接wire化。
- `echo-orchestration/src/runtime/turn_driver.rs:418` — TurnReceipt与唯一终态 — 记录完整receipt扩展义务，不在当前Plan驱动执行。
- `echo-core/src/tools/mod.rs:323` — ToolResult的非wire model_content — manifest必须显式映射，不能依赖serde skip。
- `src/agent/handle.rs:337` — 进程内AgentHandle — 扩展handle不暴露Arc/RwLock或closure escape hatch。
- `echo-integration/src/mcp/transport/stdio.rs:80` — 现有MCP stdio只路由response — 作为禁止复用其parser/demux实现ACP的负面证据。
- `echo-agent-learning/tests/documentation_contract.rs:115` — 清单唯一与fail-closed合同模式 — 复用确定性inventory/check思路。
- `scripts/verify.sh:1` — 当前本地完整Rust门禁入口 — 增量接入合同检查。
- `.github/workflows/rust-ci.yml:22` — Linux quality与分组测试结构 — 保持资源边界并增加独立合同信号。

## Todos

### freeze-facade-acp-inventory

requirements:
- § 1. 问题与目标
- § 2. 已确认决策
- § 3. 业界参考与取舍
- § 4. 当前代码事实与复用结论
- § 5. 范围与非目标
- § 7. 公共 API 权威与对等清单
- § 13. Feature 模型
- § 17. 源码布局与交付合同
- § 18. 版本与兼容策略
- § 20. 验收标准
- § 22. 当前状态

interfaces:
- consumes: 根echo_agent rustdoc可达公共面、Cargo feature graph、官方稳定ACP v1 wire/schema artifact和固定public-api/rustdoc工具链。
- produces: echo-sdk-protocol最小crate基础、acp-baseline.json、public-api.txt、parity-manifest schema/data，以及default/full/逐叶feature的确定性ACP relationship检查。

steps:

1. 核验并精确锁定支持稳定ACP wire protocolVersion 1、兼容Rust 1.95且不启用draft/unstable能力的官方schema artifact；分别记录wire、crate/schema artifact与feature集合。
   verify: acp-baseline.json不使用latest或版本范围，不复制官方schema，并能检测Cargo.lock、artifact identity和声明版本漂移。
   expected: 后续实现不会把官方crate major、schema artifact版本和ACP wire version混为一谈。
2. 固定inventory工具与专用rustdoc toolchain，按Cargo metadata展开full和全部现有叶feature，生成根echo_agent结构化公共项集合及确定性快照。
   verify: 重复生成得到相同顺序与内容；glob re-export、impl method、宏和data feature不被静默遗漏。
   expected: public-api快照可审阅真实facade边界，不依赖prelude或README手写列表。
3. 定义parity manifest机器schema，把每个公共项唯一归入语义分类，并标注standard、standard_projection、echo_extension或language_intrinsic以及三语言映射状态。
   verify: 每个inventory identity恰有一个entry；重复、stale identity、非法feature/relationship和缺少任一语言字段均稳定失败。
   expected: 后续ACP adapter、Host和语言Plan消费同一映射，标准投影损失不会冒充完整对等。

### define-echo-extension-contract

requirements:
- § 6. 系统边界与分层
- § 7. 公共 API 权威与对等清单
- § 8. SDK 对象模型
- § 9. 语言惯用 API
- § 10. 协议合同
- § 11. 事件、结果与恢复
- § 12. Extension Bridge
- § 13. Feature 模型
- § 15. 异常与边界场景
- § 16. 安全与本地边界
- § 18. 版本与兼容策略
- § 21. 关键取舍与后果

interfaces:
- consumes: acp-baseline、parity manifest分类、官方ACP扩展规则、EventEnvelope事实和现有facade值/trait语义。
- produces: echo_sdk_protocol EchoAgentCapability、EchoSdkError、WireHandle、WireU64、WireDuration、WirePath、WireBytes、完整event/replay DTO及canonical _echo_agent/* method catalog。

steps:

1. 定义ACP _meta中的namespaced echo-agent capability，包含extension protocol version、contract/source digest、实际叶feature、limits及required/optional扩展能力。
   verify: 标准ACP Client可忽略capability；SDK Client能检测缺失、版本/digest不兼容、缺必需feature和超限声明并fail closed。
   expected: 标准ACP合规性和完整SDK协商在同一initialize上保持独立。
2. 定义_echo_agent/* method catalog及其request/notification payload，覆盖Agent/Session/Run、replay/gap、Task/Subagent、extension注册调用和其余facade adapter obligation。
   verify: 所有自定义方法以下划线开头且有capability声明；catalog不包含标准ACP方法，也不定义基础JSON-RPC envelope/parser。
   expected: 后续Host只能实现已冻结扩展，并将标准ACP方法交给官方runtime。
3. 定义扩展专用错误、领域identity、handle generation与无损scalar/path/binary/event规则。
   verify: ACP数字request ID按官方类型处理；u64最大值、非UTF-8 Unix path、Windows UTF-16、纳秒Duration、binary、unknown和完整EventEnvelope均无损往返。
   expected: 领域identity不借用request ID，标准ACP限制不削弱完整SDK语义。
4. 为TurnRequest/TurnReceipt、ChatRequest/Response/Chunk、ToolResult/ToolContext、AgentHandle、closure DSL和generic facade记录独立DTO或language intrinsic义务，不批量增加serde。
   verify: 每个非wire-ready类型都有明确standard projection或echo_extension映射，且不丢字段、不暴露锁/内存地址。
   expected: adapter保持薄且无损，不建立第二套framework model。

### generate-extension-schema-fixtures

requirements:
- § 10. 协议合同
- § 11. 事件、结果与恢复
- § 12. Extension Bridge
- § 15. 异常与边界场景
- § 18. 版本与兼容策略
- § 20. 验收标准

interfaces:
- consumes: echo_sdk_protocol扩展DTO、method catalog、acp-baseline和parity manifest。
- produces: echo-agent-extension-v1.schema.json、contract/source digest、版本化extension fixtures及默认只读的deterministic generation check。

steps:

1. 仅从echo-agent扩展DTO和catalog生成版本化JSON Schema，并用canonical serialization计算可复现contract digest。
   verify: Schema不包含或重定义ACP JsonRpc/Initialize/Session/Prompt/ContentBlock/update/stop reason；同revision重复生成得到相同排序和digest。
   expected: 官方ACP schema保持外部权威，项目只维护自身扩展合同。
2. 建立覆盖capability、方法、错误、scalar、handle、event/replay、unknown、bounds和标准/扩展边界的有效与无效fixtures。
   verify: 有效扩展fixture无损往返，非法namespace、未声明method、领域ID、路径、整数和载荷被指定扩展错误拒绝；不复制官方conformance fixtures。
   expected: 后续三语言只对同一扩展输入实现，不各自解释或分叉ACP。
3. 提供显式update与默认check模式；check在隔离位置重生成并比较提交产物。
   verify: 默认模式不写仓库；显式update后再次check零diff，手工修改生成物稳定失败。
   expected: extension生成物可审阅且普通测试不会静默重写。

### gate-and-document-acp-contract

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
- consumes: inventory/ACP baseline/parity/extension schema/fixture检查入口、现有scripts/verify.sh和分组Rust CI。
- produces: scripts/check-sdk-contracts.sh、本地/CI合同门禁，以及docs/sdk唯一入口和Contract状态说明。

steps:

1. 增加统一合同检查入口并接入scripts/verify.sh和现有Linux分组CI；固定工具缺失时给出明确前置条件，不在普通检查中隐式安装。
   verify: 本地与CI对inventory、ACP baseline、manifest、extension Schema/fixtures漂移给出相同非零结果；原Rust门禁和低资源分组不被删除。
   expected: facade、ACP artifact或扩展变化无法通过只编译代码绕过合同检查。
2. 创建docs/sdk/README.md与protocol.md，说明source-only、标准ACP和完整SDK双profile、官方artifact版本、投影损失、扩展namespace及当前仅Contract状态，并从根README导航。
   verify: 文档不声称ACP Agent adapter、Host、conformance、TS/Python/Java或registry/binary已经可用，所有本地链接可解析。
   expected: 外部读者能区分Design、Contract、ACP conformant、Runnable、Parity complete和Published。
3. 执行仓库既有完整门禁及适用的逐feature条件矩阵，确认合同工具不破坏default、all-feature、featureless或单feature构建。
   verify: cargo fmt --all、./scripts/verify.sh和AGENTS.md条件矩阵全部退出0；生成检查后工作树无额外diff。
   expected: 当前outcome可独立合并并停止，后续acp-agent-adapter Plan只消费已验证合同。

## Decisions

- Plan 02取代旧Plan 01；旧Plan的私有JSON-RPC envelope、数字ID禁令、host-protocol schema和基础protocol fixtures全部关闭。
- 当前Plan只交付Contract，不新增根acp feature、ACP Agent adapter、Host、Agent/Session/Run service、ACP conformance或语言SDK。
- 官方稳定ACP v1是唯一基础协议；本仓库只固定精确artifact identity并定义_echo_agent/*扩展，不复制官方schema/runtime/conformance。
- public inventory覆盖根facade完整rustdoc可达面，不把prelude或workspace子crate内部pub项当边界。
- protocol-owned DTO只服务echo-agent扩展；标准ACP类型始终由官方语言库提供。
- contract digest与Git/source identity共同约束扩展版本；ACP wire、官方crate/schema artifact和echo extension版本分别治理。
- 第三方源码依赖可按manifest/lockfile解析；禁止的是本项目预编译Host/SDK产物，不要求vendor全部生态依赖。
- examples在当前outcome没有可运行API，保持不变；echo-website等待Parity complete后由独立outcome更新。
