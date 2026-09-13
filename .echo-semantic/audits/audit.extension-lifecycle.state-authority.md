---
schema_version: 1
id: audit.extension-lifecycle.state-authority
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: state_authority
freshness: examined
revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
finding_refs: [finding.skill-activation-authority, finding.plugin-mcp-owner-isolation, finding.plugin-failure-isolation-contract, finding.plugin-lifecycle-coordination, finding.plugin-generation-publication-authority, finding.hook-event-producer-contract]
challenges:
  skill-activation-authority:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [src/agent/react/subsystems/tool_exec.rs, src/agent/react/capabilities.rs, src/agent/react/mod.rs, src/agent/snapshot.rs, echo-execution/src/skills/external/activate_tool.rs, echo-execution/src/skills/external/resource_tool.rs]
    evidence_refs: [evidence.effects-extensions]
  plugin-mcp-owner-and-generation:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-integration/src/mcp/mod.rs, src/plugin/prepared.rs]
    evidence_refs: [evidence.effects-extensions]
  plugin-state-and-failure-isolation:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-core/src/plugin/registry.rs, echo-core/src/plugin/lifecycle.rs, src/plugin/prepared.rs, echo-sdk-host/src/core_profile/facade/source_operations.rs, docs/adr/0012-immutable-plugin-preparation.md, docs/en/32-plugin-system.md]
    evidence_refs: [evidence.effects-extensions]
---

# Extension Registry、Owner 与 Generation 状态权威审计

## 审查范围

审查 Skill activation、MCP owner identity、Plugin Registry/Prepared generation/Lifecycle callbacks、failure isolation 与生命周期 Hook producer。

## 已检查故障假设

验证双 Skill registry 是否分叉，裸 MCP name 是否跨 Plugin 错误替换/撤销，旧 generation 是否可重新发布，以及 durable enabled/live wiring/callback 是否缺 coordinator。

## 实际实现路径与证据

Catalog/progressive registry 与 invocation telemetry 形成多份 Skill activation，API/tool/checkpoint 路径不对等。MCP manager/wiring receipt 只保存裸名，旧 owner 可断开新 owner。Prepared set 有 generation，但 wiring result/active state/unwire 无 generation fence，旧 Arc 可重新发布。Registry、Integrator、LifecycleManager 各自提交状态，SDK mutation 只改 Registry。单组件错误令整代 non-applicable，与正式文档的最小隔离承诺冲突。

## 问题记录

确认四个既有 extension Finding 与 Hook producer gap；新增 Plugin generation publication authority。Failure isolation 需 semantic-decide 选择 atomic generation 或 component isolation。

## 残余风险

任何协调修复必须保持 Registry durable authority、Integrator live generation 与 Lifecycle cleanup debt 的独立职责，同时提供有序事务/补偿。

## 未检查项

未审查 EKO 是否另有 coordinator，未运行跨 registry/owner/generation 故障测试。
