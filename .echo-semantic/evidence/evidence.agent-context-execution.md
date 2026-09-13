---
schema_version: 1
id: evidence.agent-context-execution
kind: evidence
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
source_refs:
  - echo-core/src/agent/mod.rs
  - echo-core/src/agent/factory.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/react_loop.rs
  - src/agent/react/run/stream_channel.rs
  - src/agent/snapshot.rs
  - src/agent/react/run/phases/compact.rs
  - src/agent/react/run/phases/tools.rs
  - src/agent/react/run/phases/finalize.rs
  - src/agent/handle.rs
  - echo-orchestration/src/runtime/turn_driver.rs
  - src/acp/runtime.rs
  - src/acp/session.rs
  - src/headless.rs
  - src/channels.rs
  - echo-integration/src/channels/session.rs
  - echo-state/src/compression/mod.rs
  - src/context/mod.rs
  - src/state/mod.rs
  - src/state/file.rs
  - docs/adr/0001-channel-session-sender-scope.md
  - docs/adr/0005-invocation-resource-lifetime.md
  - docs/adr/0006-runtime-state-scope-lineage.md
  - docs/adr/0009-tracked-input-receipts.md
  - docs/adr/0010-canonical-turn-receipt-accounting.md
  - tests/agent_handle_turn_driver.rs
supports: [behavior.agent-turn-lifecycle, behavior.context-memory-lifecycle, rule.turn-terminal-authority, rule.context-persistence-separation]
limitations:
  - ContextAssembler 与默认 ContextManager 的策略等价性以及 AgentCheckpoint current_plan 写入路径仍需定向复核
  - Channel/direct Rust 调用不经过 AgentTurnDriver，transcript projection 失败仅告警；两者由开放 Finding 路由
---

# Agent、Turn 与 Context 证据

## 支持的结论

`ReactAgent` 是默认 Agent 实现；`AgentTurnDriver` 与 `TurnReceipt` 拥有一次 driven invocation 的序列、终态与计量；Channel/direct Rust 调用当前仍走 raw Agent execution。`ContextManager` 拥有默认 ReAct 活跃上下文；runtime checkpoint、transcript 和长期 memory 是不同持久化边界。

## 来源与范围

来源覆盖 Agent trait/实现/handle、ReAct 主循环、Turn driver、ACP/Headless/Channel 入口、snapshot producer、safe-point callers、ContextManager、RuntimeStateStore、相关 ADR 和集成测试。

## 已知缺口

仓库没有一个通用 `AgentRevision` 或统一 Session aggregate；不同 Session、Conversation、runtime incarnation 与 SDK handle 的映射在 Capability Map 中限定，不新增合并类型。
