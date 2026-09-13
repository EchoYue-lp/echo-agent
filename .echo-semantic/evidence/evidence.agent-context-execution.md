---
schema_version: 1
id: evidence.agent-context-execution
kind: evidence
observed_at: 81e2756cee9127fa23a9bb1023bd56aa8f954964
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
  - src/eval/runner.rs
  - src/channels.rs
  - echo-integration/src/channels/manager.rs
  - echo-integration/src/channels/types.rs
  - echo-integration/src/channels/session.rs
  - echo-state/src/compression/mod.rs
  - src/context/mod.rs
  - src/state/mod.rs
  - src/state/file.rs
  - src/state/sqlite.rs
  - src/agent/react/tests.rs
  - docs/adr/0001-channel-session-sender-scope.md
  - docs/adr/0005-invocation-resource-lifetime.md
  - docs/adr/0006-runtime-state-scope-lineage.md
  - docs/adr/0009-tracked-input-receipts.md
  - docs/adr/0010-canonical-turn-receipt-accounting.md
  - docs/adr/0037-eval-timeout-turn-settlement.md
  - docs/adr/0038-eval-trace-correlation-identity.md
  - docs/en/24-eval-system.md
  - docs/zh/24-eval-system.md
  - tests/agent_handle_turn_driver.rs
supports: [behavior.agent-turn-lifecycle, behavior.context-memory-lifecycle, rule.turn-terminal-authority, rule.context-persistence-separation]
limitations:
  - ContextAssembler 与默认 ContextManager 的策略等价性以及 AgentCheckpoint current_plan 写入路径仍需定向复核
  - Channel/direct Rust 调用不经过 AgentTurnDriver，transcript projection 失败仅告警；两者由开放 Finding 路由
---

# Agent、Turn 与 Context 证据

## 支持的结论

`ReactAgent`是默认Agent实现并拥有真实trace Run创建；`AgentTurnDriver`与`TurnReceipt`拥有一次driven invocation的序列、终态与计量；Eval复用该driver和value-scoped runtime correlation，在deadline后继续等待同一future的bounded settlement，并从RunStore解析真实trace而非读取product run getter。Channel/direct Rust调用当前仍走raw Agent execution。`ContextManager`拥有默认ReAct活跃上下文；runtime checkpoint、transcript、trace和长期memory是不同持久化边界。

## 来源与范围

来源覆盖Agent trait/实现/handle、ReAct主循环、Turn driver、ACP/Headless/Eval/Channel入口、value-scoped identity、snapshot/trace producer、safe-point callers、ContextManager、RuntimeStateStore、相关ADR和集成测试。

## 已知缺口

仓库没有一个通用 `AgentRevision` 或统一 Session aggregate；不同 Session、Conversation、runtime incarnation 与 SDK handle 的映射在 Capability Map 中限定，不新增合并类型。
