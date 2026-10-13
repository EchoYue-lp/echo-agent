---
schema_version: 1
id: rule.framework-layer-ownership
kind: rule
status: verified
expectation: human_confirmed
risk: high
primary_focus: state_authority
focus:
- contract_evidence
- permission_external
observed_at: source:1c515c09873968a279e6076cf6abf659923f1fa96137144423970bfcd0e1881c
behavior_refs:
- behavior.workspace-composition
code_refs:
- Cargo.toml
- src/lib.rs
- README.md
- README.zh.md
- docs/en/architecture.md
- docs/zh/architecture.md
- docs/en/concepts.md
- docs/zh/concepts.md
- docs/en/lifecycles.md
- docs/zh/lifecycles.md
- echo-agent-learning/tests/documentation_contract.rs
- docs/adr/0014-framework-capability-placement.md
- docs/adr/0040-framework-concept-documentation-authority.md
- docs/en/39-framework-application-boundary.md
- Cargo.lock
- echo-core/Cargo.toml
- echo-execution/Cargo.toml
- echo-integration/Cargo.toml
- echo-agent-learning/Cargo.toml
- docs/dependencyrepair.md
- docs/evidence/dependency-repair-20261013/receipt.json
evidence_refs:
- evidence.workspace-structure
- evidence.workspace-topology-doc-repair
- evidence.workspace-topology-doc-verification
- evidence.feature-table-doc-repair
- evidence.feature-table-doc-verification
- evidence.readme-example-target-repair
- evidence.readme-example-target-verification
- evidence.framework-concept-navigation
- evidence.framework-dependency-policy-repair
- evidence.framework-dependency-policy-focused-verification
- evidence.framework-dependency-final-full-verification
- evidence.framework-dependency-final-gate-archive
finding_refs:
- finding.framework-dependency-policy-ci
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


本轮依赖边界：六Cargo文件的maintained YAML alias与quinn锁替换属于框架编译实现，复用原公共API与调用路径，原audit/deny门禁权威保持；focused/policyredgreen不代新全量与交付验收。旧已verified分层规则未改为其它risk/依赖全部通过。


依赖本地修复收口：新23完整工程最终通过及独立源码review PASS/action0见 evidence.framework-dependency-final-full-verification / audit.framework-dependency-policy-rereview，原分层/状态不扩展为remote/consumer/全risk已验。
