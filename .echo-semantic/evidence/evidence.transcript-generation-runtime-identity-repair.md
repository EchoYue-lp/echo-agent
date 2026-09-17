---
schema_version: 1
id: evidence.transcript-generation-runtime-identity-repair
kind: evidence
observed_at: source:469a276a3666fa7b9f836bc4c5751516ca360fcdb16efae6cc81c2d66ffb2560
source_refs:
  - src/agent/snapshot.rs
  - src/agent/react/run/stream_channel.rs
  - src/state/mod.rs
  - docs/adr/0001-channel-session-sender-scope.md
  - docs/en/41-persistence-concepts.md
  - docs/zh/41-persistence-concepts.md
supports: [behavior.context-memory-lifecycle, rule.context-persistence-separation, finding.transcript-generation-runtime-identity]
limitations:
  - full integration gate, independent rereview and remote CI remain pending
  - the repair does not settle ConversationStore projection failures tracked by finding.transcript-projection-settlement
---

# Runtime state 与 transcript generation identity 修复证据

## 支持的结论

一个 crate-private resolver 现在统一 stream restore 与 snapshot save 的 runtime identity 优先级：
显式 invocation runtime ID、invocation 产品 conversation、legacy external conversation、最后是
configured conversation。若 `transcript_generation_id` 存在且与该有效 identity 不相等，stream
在 execution mutex、guard、trace、context、LLM 和 checkpoint 副作用前失败关闭。

`AgentRunSnapshot::save_runtime_checkpoint` 在读取 context、序列化 payload 或调用 Store 前再次
执行同一校验，因此直接构造 public snapshot 也不能绕过 admission 写出不可恢复 checkpoint。
未提供 transcript generation 的既有调用保持兼容；相等 identity 的保存和重启路径不变。

## 来源与范围

修复只改变 framework 内部 admission/checkpoint invariant，不新增 public API、store、schema、
feature 或 SDK wire contract。恢复端对既有损坏 checkpoint 的 fail-closed 校验继续保留。

## 已知缺口

本 Evidence 不处理 transcript projection backend 的失败、timeout、ambiguous commit 或 durable debt；
这些属于独立 Finding #106。
