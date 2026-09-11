---
schema_version: 3
lifecycle: completed
supersedes: null
slug: 2026-09-04-source-first-multilanguage-sdk-runtime/plan
goal: 为根 echo_agent 的全部正式公共 operation、stream 和 consumer-trait extension 建立可执行的
  canonical route 或 typed bridge，消除 generic source-operation fail-closed 与未证明
  intrinsic 分类。
ships: SDK Host 为剩余 root facade operation、consumer trait 和 stream surface 提供
  Rust 权威驱动的 typed adapter/ExtensionBridge、统一 catalog、WireHandle
  生命周期和跨语言合同验证；不重新实现 Agent 核心。
verify: 从源码构建的 sdk-facade-all Host 能对每个 parity manifest canonical operation 提供真实
  handler、typed bridge 或有证据的 language intrinsic；generic source-operation 不再无条件落入
  feature_unavailable，consumer trait route 与 proxy
  逐项对账，stream/handle/owner/generation/close/背压测试通过，合同漂移、单 feature、全 workspace
  门禁和 facade E2E 全部通过。
design_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/design.md
delivery_ref: docs/supreme/specs/2026-09-04-source-first-multilanguage-sdk-runtime/plans/delivery-map.md#facade-public-api-parity
todos:
  - id: freeze-source-operation-routes
    files:
      - echo-sdk-protocol/src/inventory.rs
      - echo-sdk-protocol/src/facade.rs
      - echo-sdk-host/src/core_profile/facade/registry.rs
      - echo-sdk-protocol/tests/facade_inventory.rs
      - contracts/sdk/facade-operation-catalog.json
      - contracts/sdk/parity-manifest.json
    summary: 按 canonical source identity 将每个 public operation 归入真实
      family/core/bridge/intrinsic route，禁止无 handler generic route。
    verify: catalog 每个 canonical operation 都有唯一
      route、signature、feature、输入/结果描述和验证引用，source-operation 不再批量落入无实现 invoke。
  - id: bridge-remaining-consumer-traits
    files:
      - echo-sdk-protocol/src/methods.rs
      - echo-sdk-protocol/src/catalog.rs
      - echo-sdk-host/src/core_profile/extension_bridge.rs
      - echo-sdk-host/src/core_profile/facade/mod.rs
      - echo-sdk-host/tests/extension_bridge_e2e.rs
      - echo-sdk-protocol/tests/extension_contract.rs
    summary: 为仍有 Host live consumption point 的 consumer trait 增加 typed
      ExtensionKind、DTO、proxy、取消、超时和断连语义；其余 intrinsic 必须有逐项证据。
    verify: 每个 canonical extension trait 都有 typed bridge、family adapter 或可复核
      intrinsic 理由；成功、typed failure、timeout、cancel、disconnect 和 late response
      覆盖。
  - id: implement-source-operation-adapters
    files:
      - echo-sdk-host/src/core_profile/facade/mod.rs
      - echo-sdk-host/src/core_profile/facade/source_operations.rs
      - echo-sdk-host/src/core_profile/handler.rs
      - echo-sdk-host/tests/core_profile_e2e.rs
      - echo-sdk-host/tests/facade_feature_adapters_e2e.rs
    summary: 复用 Agent/Session/Run/Task/Subagent/Store/Workflow 等既有 Rust
      authority，为可远程 public operation 提供薄 typed handler 和统一 receiver/resource
      校验。
    verify: 官方 ACP Client 可真实调用 canonical source-operation，结果、错误、取消、恢复和终态与 Rust
      framework authority 一致；未知/未编译 operation fail closed。
  - id: unify-facade-stream-authority
    files:
      - echo-sdk-host/src/core_profile/handles.rs
      - echo-sdk-host/src/core_profile/facade/stream.rs
      - echo-sdk-host/src/core_profile/facade/mod.rs
      - echo-sdk-protocol/src/handle.rs
      - echo-sdk-host/tests/facade_feature_adapters_e2e.rs
    summary: 将 facade stream 接入统一 WireHandle registry，完成
      owner/generation/sequence/ack/cancel/close/backpressure 生命周期。
    verify: 每个 public facade stream 都使用 Host-issued WireHandle；旧代、错 kind、跨
      Session、重复 close、背压和 teardown 均有 typed 行为。
  - id: prove-public-api-parity-closeout
    files:
      - echo-sdk-host/tests/facade_feature_adapters_e2e.rs
      - echo-sdk-protocol/tests/facade_inventory.rs
      - docs/sdk/facade-feature-adapters.md
      - docs/sdk/protocol.md
      - docs/sdk/README.md
      - docs/adr/0028-source-first-multilanguage-sdk-runtime.md
      - README.md
      - README.zh.md
      - CHANGELOG.md
      - scripts/check-sdk-contracts.sh
      - .github/workflows/rust-ci.yml
    summary: 完成逐项 parity 报告、文档、CI 和最终门禁，明确 Design/Contract/Runnable/Parity complete 状态。
    verify: 合同无漂移、CI 执行完整 facade 矩阵、单 feature 与 full facade 全绿；只有所有 canonical public
      API 证据齐全后才更新 Plan/Delivery 状态。
artifact_id: plan:a50f8ef7-4307-4b25-8c8b-9f0f939c28d4
design_revision: sha256:21ebbfd71662b6de98246f8dbbf0e2ed2d863cae0da14bf0af328a80d76dcaf5
---
## Context

Plan 07 已交付 Rust Host 的主要 feature-family adapters，但审查仍发现 source-operation generic routes、部分 consumer traits 和 facade stream lifecycle 没有达到 Design §7.2 的完整公共面合同。本 Plan 继承同一 design revision，继续补齐剩余公共面，不把 fail-closed 当作 parity 完成。

## Approach

- 先以 inventory/source identity 建立唯一 canonical operation registry，逐项证明 remote handler、typed bridge 或 language intrinsic；不再使用批量 fallback。
- consumer trait 只在 Host 有真实消费点时扩展 typed bridge；proxy 复用现有 ExtensionBridge invocation authority。
- source operation handler 只做 DTO/handle/feature 转换，执行、重试、取消、恢复和终态继续由 Rust authority 决定。
- stream 与 resource 共用 HandleRegistry，不保留第二套 generation/owner/close authority。

## Global Constraints

- ACP v1 和 _echo_agent namespace 不变；不引入第二 JSON-RPC、第二执行器或语言 runtime。
- Rust 是唯一执行权威；不得在 Host 或 SDK 中复制 Agent/Task/Subagent 状态机。
- 不得以 feature_unavailable、intrinsic 或空 handler 掩盖未完成的 public API；每项必须有证据。
- 不改变 EKO 应用层、echo-agent-cli、echo-website 或源代码优先交付边界。
- 所有新增公开 contract、trait kind、stream 或错误字段必须同步 schema、fixtures、docs、examples 和 SDK-Docs-Impact/SDK-Skill-Impact 结论。

## Files

- Modify: `echo-sdk-protocol/src/inventory.rs` — 生成逐项 canonical source-operation route 和分类证据。
- Modify: `echo-sdk-protocol/src/facade.rs` — 冻结 operation route、signature、feature metadata。
- Modify: `echo-sdk-protocol/src/methods.rs` — 冻结 typed source-operation 与 trait bridge DTO。
- Modify: `echo-sdk-protocol/src/catalog.rs` — 绑定 operation/bridge/stream catalog。
- Modify: `echo-sdk-host/src/core_profile/facade/registry.rs` — 执行 canonical operation registry。
- Modify: `echo-sdk-host/src/core_profile/facade/mod.rs` — 接入 source-operation admission/dispatch。
- Create: `echo-sdk-host/src/core_profile/facade/source_operations.rs` — 承载薄 source-operation adapter。
- Modify: `echo-sdk-host/src/core_profile/extension_bridge.rs` — 增加剩余 live consumer trait proxies。
- Modify: `echo-sdk-protocol/src/handle.rs` — stream handle kind contract.
- Modify: `echo-sdk-host/src/core_profile/handles.rs` — 扩展 stream/resource authority。
- Modify: `echo-sdk-host/src/core_profile/facade/stream.rs` — 统一 WireHandle stream lifecycle。
- Modify: `echo-sdk-host/src/core_profile/handler.rs` — 接入 close/teardown authority。
- Modify: `echo-sdk-host/tests/core_profile_e2e.rs` — source-operation behavior coverage。
- Modify: `echo-sdk-host/tests/extension_bridge_e2e.rs` — remaining trait bridge coverage。
- Modify: `echo-sdk-host/tests/facade_feature_adapters_e2e.rs` — stream/handle parity coverage。
- Modify: `echo-sdk-protocol/tests/facade_inventory.rs` — route completeness coverage。
- Modify: `echo-sdk-protocol/tests/extension_contract.rs` — trait bridge contract coverage。
- Modify: `contracts/sdk/facade-operation-catalog.json` — generated operation catalog。
- Modify: `contracts/sdk/parity-manifest.json` — generated public parity manifest。
- Modify: `docs/sdk/facade-feature-adapters.md` — route/resource/stream status。
- Modify: `docs/sdk/protocol.md` — operation and bridge contract。
- Modify: `docs/sdk/README.md` — status ladder。
- Modify: `docs/adr/0028-source-first-multilanguage-sdk-runtime.md` — architecture decision。
- Modify: `README.md` — public status。
- Modify: `README.zh.md` — 中文 public status。
- Modify: `CHANGELOG.md` — delivered behavior。
- Modify: `scripts/check-sdk-contracts.sh` — contract gate。
- Modify: `.github/workflows/rust-ci.yml` — complete facade CI matrix。

## Reuse

- echo-sdk-protocol/src/inventory.rs — 现有 rustdoc inventory/source identity/signature extraction。
- echo-sdk-host/src/core_profile/extension_bridge.rs — 现有 lease/deadline/cancel/stream settlement authority。
- echo-sdk-host/src/core_profile/handles.rs — 现有 generation/owner/closed/tombstone ladder。
- echo-agent/src/acp/runtime.rs、TaskRevisionService、RuntimeTaskService、SubagentExecutor、EventJournal、RunStore、Workflow — 既有 Rust semantic authorities。

## Todos

### freeze-source-operation-routes

requirements:
- § 7.1 权威集合
- § 7.2 Parity manifest
- § 10.4 echo-agent SDK extension families
- § 10.6 错误合同
- § 13 Feature 模型
- § 20.1 公共面完整性

interfaces:
- consumes: rustdoc inventory、FacadeFamily、ExtensionKind、现有 family/core/bridge routes
- produces: 逐项 canonical operation catalog、route classification 和 handler obligations

steps:

1. 统计当前 invoke/family/value/intrinsic/bridge 分布，按 source identity 和 alias group 检查每个 canonical operation。
   verify: 生成报告列出每项唯一 route、feature、signature、handler/proxy/intrinsic evidence。
   expected: 不存在没有 handler 或具体 intrinsic 理由的 canonical remote operation。
2. 将 route metadata 从聚合 family item 扩展为 operation-level typed record，并补齐 schema/fixtures。
   verify: catalog/parity/schema round-trip 和 unknown/signature/feature/receiver failure 全部通过。
   expected: Host registry 能按 operation 精确查找执行契约。

### bridge-remaining-consumer-traits

requirements:
- § 7.2 Parity manifest
- § 10.4 echo-agent SDK extension families
- § 12.1 统一模型
- § 12.2 Trait 映射
- § 12.3 并发与死锁约束
- § 20.3 行为一致性

interfaces:
- consumes: ExtensionInvocation authority、existing DTO/proxy patterns、framework live consumers
- produces: typed ExtensionKind/proxy/contract tests or evidence-backed intrinsic classification

steps:

1. 按 live consumption point 对剩余 traits 分组，禁止把行为 trait 仅因未实现而归为 intrinsic。
   verify: 每组都有 Host call path、proxy、failure/cancel/close test 或明确 process-local evidence。
   expected: manifest route 和 TYPED_BRIDGE_TRAITS 一致。
2. 接入 typed bridge DTO、registration、proxy、deadline、cancel、disconnect 和 late response。
   verify: extension contract 与真实 Host E2E 覆盖成功、拒绝、超时、取消和断连。
   expected: SDK callback failure 不触发内置 fallback。

### implement-source-operation-adapters

requirements:
- § 8 SDK 对象模型
- § 10.4 echo-agent SDK extension families
- § 11 事件、结果与恢复
- § 14 生命周期与数据流
- § 20.3 行为一致性

interfaces:
- consumes: canonical operation registry、HandleRegistry、Session/Run/Task/Subagent/Store/Workflow authorities
- produces: typed source-operation handlers、receiver/resource conversions、operation E2E

steps:

1. 按业务能力分组 Agent/Session/Run/Task/Subagent/Store/Workflow 等 source operations，明确每组 wire DTO 和 Rust authority。
   verify: 每组至少有成功、typed failure、cancel/close/recovery 观测。
   expected: source operation 不再统一落入 generic feature_unavailable。
2. 通过 official connection task 挂载 handlers，并补跨语言可消费的 schema/fixtures。
   verify: standard/core/bridge/facade profiles 回归，stdout/teardown/backpressure 边界保持。
   expected: generic invoke 或 typed family surface 都能到达真实 handler。

### unify-facade-stream-authority

requirements:
- § 8 SDK 对象模型
- § 10.5 路径、数值与投影边界
- § 11.2 Replay 与背压
- § 12.3 并发与死锁约束
- § 20.4 可靠性

interfaces:
- consumes: HandleRegistry、FacadeResource owner records、existing extension stream sink/backpressure
- produces: public facade WireHandle stream lifecycle and E2E evidence

steps:

1. 将 FacadeStreamBookkeeping 改为 HandleRegistry-backed stream records，统一 shape/kind/generation/owner/sequence/cancel/close。
   verify: stale/wrong-kind/foreign/repeated-close/backpressure tests fail closed.
   expected: 不存在第二套 facade stream handle authority。
2. 为真实 stream-producing family 接入 open/return/control/cancel/close。
   verify: stream result、terminal exactly-once、connection/session teardown 和 replay watermark 通过真实 Host。
   expected: stream contract 可被三语言 SDK直接消费。

### prove-public-api-parity-closeout

requirements:
- § 7 公共 API 权威与对等清单
- § 18 版本与兼容策略
- § 19 文档与示例
- § 20 验收标准

interfaces:
- consumes: canonical catalog、bridge matrix、source-operation handlers、stream authority
- produces: complete parity evidence、docs/CI/status update and impact records

steps:

1. 运行合同生成、单 feature/full facade/bridge/core/stdio/workspace gates，读取完整退出码和失败数。
   verify: all applicable commands pass with zero drift/warnings/failures.
   expected: Plan 08 can only complete when every canonical public item has evidence.
2. 更新 docs、delivery map、Plan lifecycle、SDK-Docs-Impact 和 SDK-Skill-Impact。
   verify: status claims exactly match evidence; no language SDK Runnable/Parity claim before their own outcomes.
   expected: downstream TypeScript/Python/Java outcomes have a truthful dependency boundary.
