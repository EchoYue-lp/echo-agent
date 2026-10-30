---
schema_version: 1
id: asset.agent-factory-contracts
kind: asset
title: 两类 AgentFactory 合同
asset_type: symbol
status: active
risk: medium
observed_at: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
boundary_refs:
- boundary.agent-session-turn
- boundary.workspace-architecture
code_refs:
- echo-core/src/agent/factory.rs
- src/agent/subagent/registry.rs
consumer_refs:
- src/agent/default_factory.rs
- src/agent/subagent/executor.rs
behavior_refs:
- behavior.agent-turn-lifecycle
- behavior.task-subagent-execution
rule_refs:
- rule.framework-layer-ownership
- rule.task-subagent-authority
evidence_refs:
- evidence.agent-context-execution
- evidence.task-subagent-workflow
- evidence.factory-background-driven-source-review
- evidence.subagent-factory-singleflight-verification
finding_refs: []
candidate_refs: []
---

# 当前代码身份更新

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

已采用的三菜单完整角色、caller-owned background与SDK独立迁仓历史关系不改。本次只随当前正式代码稳定后更新正常来源；旧f8观察/原代码源审范围在下文保留，不倒签旧工程或消费者。

# 既有菜单及分范围记录

# 两类 AgentFactory 合同

## 资产身份

Core config-based Agent factory 与 Subagent registry factory 接受不同输入、承担不同构造生命周期，作为两个独立公共菜单保留；本次不因同名或单个应用未调用而合并或删除。

## 来源与消费者

DefaultAgentFactory 消费同步 `create_agent(config)`；SubagentRegistry/Executor 消费捕获配置的异步 `create()`。本轮限定源码复核及原 R20 具名观察由 evidence.factory-background-driven-source-review 单独记录；旧 Evidence 保留各自原观察范围。

## 生命周期

直接构造消费调用方配置。Registry `get_agent` 对每条记录以 OnceCell 缓存成功结果，错误或取消后可重试，发布前复核 revision/cell；选定 registration 的 factory-backed dispatch 创建 fresh instance，预建记录复用原实例，`create_fresh_agent` 继续是显式菜单。调用方拥有构造 deadline 和实例后续执行、关闭；registry 不补偿任意 factory 自建的外部副作用。

## 候选关系

历史曾候选审查长期限定命名及兼容影响；本次已核当前输入、缓存和 dispatch 合同不同，当前源码接管不要求统一接口，也不关闭原 Discovery 的长期命名议题。

## 未知与限制

原 observed_at 为 f1e9027246760661144786e9e35615cd46d580c6，原“尚未形成 consolidation 决策”只保留为该历史观察；本轮接管范围是上述当前菜单和原权威。observed_at 绑定原校验器 source_digest(root) 对当前 Framework 返回的实际来源；它只表示内容身份，不代替正常引用/source/final、SDK 或 native 资格。

当前职责只作限定 source review；第三方 factory 的外部进程/自行 spawn 的任务清理、递归 resolve 错误用法、真实模型服务、跨重启与远端环境未由本次验收。无内置任意 factory deadline 承诺，调用方需遵守原约束；这些边界不等于第三方清理已验证，也不能据本资产接管关闭其它 Finding、Rule 或 Discovery。
