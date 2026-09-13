---
schema_version: 1
id: rule.framework-layer-ownership
kind: rule
status: verified
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus: [contract_evidence, permission_external]
observed_at: 78b9f06b4320531fd8f41260887cd69c1343e995
behavior_refs: [behavior.workspace-composition]
code_refs: [Cargo.toml, src/lib.rs, docs/adr/0014-framework-capability-placement.md, docs/en/39-framework-application-boundary.md]
evidence_refs: [evidence.workspace-structure]
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
