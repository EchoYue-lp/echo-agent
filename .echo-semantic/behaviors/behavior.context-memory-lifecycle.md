---
schema_version: 1
id: behavior.context-memory-lifecycle
kind: behavior
status: needs_review
expectation: inferred
risk: high
primary_focus: data_durability
focus: [state_authority, time_lifecycle, failure_concurrency, contract_evidence]
boundary: boundary.context-memory
observed_at: source:6fdcc1782b7aa2c97a148f7c377d60b4ed26b9d470f02c05a651de1e8e2eac74
code_refs: [echo-state/src/compression/mod.rs, src/context/mod.rs, src/agent/snapshot.rs, src/agent/react/run/context.rs, src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/tools.rs, src/agent/react/run/phases/finalize.rs, src/evolution/layer.rs, src/evolution/recall.rs, src/state/mod.rs, src/state/file.rs, src/state/sqlite.rs, echo-core/src/memory/conversation.rs, echo-core/src/memory/store.rs, echo-core/src/memory/types.rs]
rule_refs: [rule.context-persistence-separation]
evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation, evidence.transcript-generation-runtime-identity-repair, evidence.transcript-generation-runtime-identity-verification, evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification, evidence.managed-import-generation-repair, evidence.checkpoint-plan-authority-repair, evidence.checkpoint-plan-authority-verification, evidence.memory-provenance-authority-repair, evidence.memory-provenance-authority-verification]
finding_refs: [finding.transcript-projection-settlement, finding.transcript-generation-runtime-identity]
---

# Context、Memory 与 Checkpoint 生命周期

## 重要承诺

活跃模型 context、runtime checkpoint、用户 transcript 和长期知识使用不同 authority；压缩只改变模型可见窗口，不重写事实历史。

## 当前行为

默认 ReAct 使用 `ContextManager`；`ContextAssembler` 是自定义 loop building block。`RuntimeStateStore` 保存 `AgentCheckpoint`，`ConversationStore` 保存 transcript，`Store` 保存长期知识。Typed memory由MemoryLayerManager持久化Draft/approval并经统一Recall资格检查。

## 期望行为

Prepare/compact/save/restore/reset/clear/delete 必须按 scope 与 generation 工作，assistant tool call/result 配对和 transcript cursor 不得跨 incarnation 混写。Managed transcript 替换应在同一 Store 事务内建立导入 generation frontier；旧 runtime scope 只能凭匹配导入回执迁移到下一 epoch。

## 触发、结果与副作用

LLM 调用前执行预算准备与压缩，turn safe point 保存 checkpoint/transcript，显式 reset/clear/delete 回收相应范围。

## 失败、重试与恢复

损坏 checkpoint、部分 transcript 写入、切换 runtime ID、重复 safe point 和取消中的 hydration 必须保守失败或重建。导入已提交但 checkpoint CAS 中断时，调用方必须重放精确导入请求，取得 AlreadyApplied 后才恢复；不允许猜测新 epoch。Transcript effect 先持久化 pending，再以稳定 operation identity apply/proof-ack；timeout 不推断未提交，admission/recovery 先结算 debt。Memory approval绑定原Draft journal generation；取消/失败后由原manager恢复结算，旧无来源记忆不自动召回。

## 证据

ContextManager、RuntimeStateStore、file backends、ConversationStore、Store、ADR 0004/0006 与恢复测试提供已知证据。

## 裁决记录

Transcript projection 失败结算已由 ADR 0056 与 Finding #106 的 framework outcome 闭合；当前仍需复核
ContextAssembler 与默认路径的策略关系。旧 `AgentCheckpoint.current_plan` 的生产恢复与写回
已退役；公开字段及 File/SQLite 读写保留，TaskRevisionService 为计划图唯一权威。
