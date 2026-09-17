---
schema_version: 1
id: evidence.transcript-projection-settlement-repair
kind: evidence
observed_at: source:df3909bab5e6d047cac27c29ce098a331020e28886a022e3daa383ac12e985f1
source_refs:
  - echo-core/src/memory/conversation.rs
  - echo-state/src/memory/file_conversation.rs
  - echo-state/src/memory/sqlite_conversation.rs
  - src/state/mod.rs
  - src/state/file.rs
  - src/state/sqlite.rs
  - src/agent/snapshot.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/context.rs
  - src/agent/react/run/stream_channel.rs
  - src/agent/react/run/phases/compact.rs
  - src/agent/react/run/phases/tools.rs
  - src/agent/react/run/phases/finalize.rs
  - docs/adr/0056-durable-transcript-projection-settlement.md
supports: [finding.transcript-projection-settlement, behavior.context-memory-lifecycle, behavior.observation-persistence, rule.context-persistence-separation, rule.fact-projection-separation]
limitations:
  - Independent SDK protocol, Host bridge and TypeScript/Python/Java contracts remain a separate Issue 106 outcome
  - Legacy unmanaged Store helpers retain their pre-existing direct-await behavior and reject context-aware deadline calls
---

# Transcript projection durable settlement repair

## 支持的结论

Framework 现在只有一个 store-backed transcript settlement coordinator。`RuntimeStateStore` 通过
revision/CAS 保存完整 `PendingTranscriptProjection`、attempt result、proof acknowledgement、generation
tombstone 与 scope retirement manifest；`ConversationStore` 通过 epoch-fenced atomic apply/delete receipt
确认已提交 transcript fact。相同 operation identity 可在 timeout、lost ack、restart 与并发 Agent 后重放。

配置 `ConversationStore` 必须同时配置支持 RevisionedV1 与 AbsoluteDeadlineV1 的
`RuntimeStateStore`，否则 admission 在 guard、trace、context、LLM 和 Store effect 前拒绝。Pre-compact、
tool、guard、hook、provider failure、cancel、consumer disconnect、NoResponse、max iteration、direct 与
stream terminal 都先产生 typed settlement；Blocked/Conflict 抑制原业务 terminal。

Exact managed clear 复用同一 coordinator 后退役 runtime authority，不猜测独立 transcript delete
identity。Managed product delete 要求调用方保留 `ManagedConversationDelete`，并通过 durable scope
manifest、完整 dropped operation set 与 retention receipt 防止 lost-ack/recreate 误删新 epoch。

## 来源与范围

实现位于 framework 的既有 ConversationStore、RuntimeStateStore、Agent snapshot 与统一 ReAct phase
边界；没有新增第二 Outbox、Task 状态机或 EKO 产品投影。File 与 SQLite 是相同 public contract 的
backend；应用只选择非零 settlement timeout 和具体 Store。

## 已知缺口

本 Evidence 只关闭 framework Finding。独立 SDK 尚需无损映射 call context、batch、receipt、settlement
event 与 retirement operation；Issue #106 在 SDK 和跨仓证据合并前保持开放。
