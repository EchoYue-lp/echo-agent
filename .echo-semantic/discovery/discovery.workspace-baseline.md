---
schema_version: 1
id: discovery.workspace-baseline
kind: discovery
source_snapshot:
  base_revision: f7a4df6a3de6d0538d6ea93a868006da07ed391d
  content_digest: 8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
scope: echo-agent 11-package workspace 的架构、入口、状态权威、生命周期、副作用、协议、持久化和验证消费者
inspected_paths: [Cargo.toml, src, echo-core, echo-execution, echo-integration, echo-macros, echo-orchestration, echo-state, echo-tools, echo-sdk-protocol, echo-sdk-host, contracts, sdks, tests, echo-agent-learning, docs, scripts, .github]
candidate_refs: [map.workspace-architecture, map.agent-session-turn, map.context-memory, map.task-subagent-workflow, map.observation-persistence-delivery, map.tool-permission-sandbox, map.extension-lifecycle, map.llm-provider-runtime, map.protocol-surfaces, map.eval-evolution]
unresolved:
  - Procedural macro、Plugin、Skill、Hook、MCP config 与 declarative workflow 的全部动态 consumer 闭集
  - AgentRevision 不存在时各限定 revision/generation 的统一 glossary 边界
  - 两类 AgentFactory 的长期限定命名与兼容影响
  - ContextAssembler 与默认 ContextManager 的策略对齐或明确差异
  - AgentCheckpoint.current_plan 的 production writer 与 canonical Task artifact 关系
  - Scheduler callback 的 delivery guarantee 与 Workflow checkpoint 消费失败后的恢复语义
  - 全部 event family 的 durable、versioned、lossy、diagnostic 与 replay 分类
  - K8s stream consumer drop 是否遗留 Pod 的真实故障注入证据
  - Plugin registry、prepared wiring、lifecycle callback 与 host shutdown 的统一 production coordinator
  - 快速变化的 provider/model facts 由 framework、provider 或 application 更新的 precedence
  - 4076 个 SDK intrinsic identity 的 capability 分组和外部用户价值
  - Evolution 自动维护、proposal、human-approved mutation 与 application scheduling 的完整协调
  - Improve 共享临时路径在并发与提前达标时的隔离/cleanup 语义
  - MCP、QQ、飞书等 credential-bearing config 的 Debug/redaction contract
  - BackgroundTaskState checkpoint abstraction 与当前 Task/CommandCell runtime 的长期关系
  - Subagent physical attempt 与内部 hook recovery ordinal 的正式术语
  - Direct-user terminal/MCP 与 Device sync 的应用侧行为仅有边界证据
---

# 全 Workspace 基线发现

## 扫描范围

扫描所有 Git 顶层区域、Cargo DAG/features、root/public/binary/background 入口、state/store/registry、外部 effect、protocol、tests/examples/docs/ADR 与现有 SDK semantic 子图。

## 候选事实

候选事实按独立 trigger、state authority、lifecycle、side effect 和 decision condition 归入十个顶层边界；API identity 只保留在 SDK inventory。

## 归并结果

形成 closed path inventory、十张顶层 map、canonical Assets、Behavior/Rule/Evidence 和当前 open Findings。Task/Workflow、Permission/Hook、Plugin/MCP/LSP、Provider/A2A/Eval/Evolution 等冲突均未在 discovery 阶段修业务代码。

## 未决项

上列 unknown 均有下一阶段 audit 路由。未执行真实 Docker/K8s、外部 provider、A2A conformance 或应用 direct-user surface，因此不从静态搜索推断它们不存在或已正确。
