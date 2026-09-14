---
schema_version: 1
id: audit.framework-concept-navigation-rereview
kind: audit
boundary_ref: boundary.workspace-architecture
lens: contract_evidence
freshness: examined
revision: 1cb25e80515ea17624fe652be1fd29c096b9a880
finding_refs: []
challenges:
  package-layer-and-application-boundary:
    revision: 1cb25e80515ea17624fe652be1fd29c096b9a880
    source_refs: [Cargo.toml, src/lib.rs, docs/en/architecture.md, docs/zh/architecture.md, docs/adr/0040-framework-concept-documentation-authority.md]
    evidence_refs: [evidence.workspace-structure, evidence.framework-concept-navigation]
  identity-state-and-persistence-authority:
    revision: 1cb25e80515ea17624fe652be1fd29c096b9a880
    source_refs: [docs/en/concepts.md, docs/zh/concepts.md, docs/en/41-persistence-concepts.md, docs/zh/41-persistence-concepts.md]
    evidence_refs: [evidence.framework-concept-navigation, evidence.agent-context-execution, evidence.persistence-observation, evidence.task-subagent-workflow]
  lifecycle-and-open-limitations:
    revision: 1cb25e80515ea17624fe652be1fd29c096b9a880
    source_refs: [docs/en/lifecycles.md, docs/zh/lifecycles.md, src/agent/react/run/phases/finalize.rs, src/agent/react/run/pipeline.rs, echo-execution/src/tools.rs, echo-core/src/plugin/lifecycle.rs]
    evidence_refs: [evidence.framework-concept-navigation, evidence.effects-extensions, evidence.agent-context-execution]
  bilingual-structure-and-executable-routes:
    revision: 1cb25e80515ea17624fe652be1fd29c096b9a880
    source_refs: [README.md, README.zh.md, docs/en/README.md, docs/zh/README.md, echo-agent-learning/tests/documentation_contract.rs, echo-agent-learning/tests/example_contracts.rs]
    evidence_refs: [evidence.framework-concept-navigation]
---

# Framework concept navigation 独立复审

## 审查范围

复审11-package DAG、root facade、SDK protocol/Host、learning consumer和application边界，核心identity/owner/non-responsibility，Context/持久化分层，Agent Turn、Task/Subagent、Tool/Permission、Observation/Delivery、Extension、SDK生命周期，双语结构、导航、example路由和ADR0040。

## 已检查故障假设

验证文档是否发明通用AgentRevision/全局Run/第二执行角色，是否将Plan/Todo/Trace/UI当作Task或Turn权威，是否固定producer/terminal/projection/close顺序，是否用checkpoint成功推导一个独立transcript write，是否把direct ToolManager当作PermissionService路径，是否泛化extension generation/cleanup/close保证，以及双语同时删节/表/diagram或交换导航顺序是否仍会假绿。

## 实际实现路径与证据

Architecture、Core Concepts与Lifecycles三对文档已发布。Architecture以Cargo/root facade为分层事实；Concepts将执行、持久化、观测和交付owner分开；Lifecycles使用owner-defined顺序，明确producer error可影响TurnReceipt而best-effort transcript不独立影响，分开automatic React Permission pipeline与direct caller policy，并将extension generation/cleanup/close收窄为component-specific。

Documentation contract冻结三对文档的heading profile与非零table/fence结构，在限定section内检查4个导航源的exact-once顺序，并以独立反例拒绝双侧同时删除和导航换序。表格计数排除fence内ASCII diagram。结构合同red精确命中6个缺失文档和12个导航链接，发布后focused 2 tests通过。

## 问题记录

首转review发现5个Important：Turn顺序、Context write分支、Tool permission路径、Extension generation/cleanup承诺与结构合同假绿。定向复审再发现automatic Tool validation顺序和checkpoint error propagation两个Important。所有问题已按真实调用路径和反例修正；最终review为PASS，Critical、Important、Minor均为0。

## 残余风险

自然语言内容不能由结构测试完全证明；外部业界链接和所有详细领域章节未逐页重审。当前71个open Finding仍限制对adapter close、terminal/projection order、transcript settlement、permission precedence和extension lifecycle等能力的承诺。

## 未检查项

未执行docs.rs发布渲染、全workspace合并门禁和远程CI；review后相关工程/Issue门禁均已通过。
