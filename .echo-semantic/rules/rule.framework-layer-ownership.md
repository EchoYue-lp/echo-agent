---
schema_version: 1
id: rule.framework-layer-ownership
kind: rule
status: verified
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [contract_evidence, permission_external]
observed_at: source:5270c224062b032f301970c2ea51e5f335a72a7e3d2a4581baf38b53cbffc23d
behavior_refs: [behavior.workspace-composition]
code_refs: [Cargo.toml, src/lib.rs, README.md, README.zh.md, docs/en/architecture.md, docs/zh/architecture.md, docs/en/concepts.md, docs/zh/concepts.md, docs/en/lifecycles.md, docs/zh/lifecycles.md, echo-agent-learning/tests/documentation_contract.rs, docs/adr/0014-framework-capability-placement.md, docs/adr/0040-framework-concept-documentation-authority.md, docs/en/39-framework-application-boundary.md]
evidence_refs: [evidence.workspace-structure, evidence.workspace-topology-doc-repair, evidence.workspace-topology-doc-verification, evidence.feature-table-doc-repair, evidence.feature-table-doc-verification, evidence.readme-example-target-repair, evidence.readme-example-target-verification, evidence.framework-concept-navigation]
finding_refs: []
---

# Framework 分层权威

## 不变量或唯一权威

通用 Agent 机制由 `echo-agent` 拥有；产品 Workspace、DomainProfile、review/worktree policy 和 UI projection 由 embedding application 拥有。

## 适用行为

适用于 crate placement、public facade、应用 adapter、公共 API 增删和状态权威迁移。

## 当前实现

Root facade 组合 split crates；应用可注入 policy/metadata 并投影结果，但 framework 不依赖 EKO 类型。

## 期望行为

应用 adapter 不得拥有第二 mailbox/store/DAG/retry/terminal authority；framework public option 不因单个应用未采用而删除。

## 证据

ADR 0014、framework boundary 文档、Cargo DAG 与 facade smoke 支持该规则。

## 裁决记录

用户已将该分层写入仓库最高优先级约束。
