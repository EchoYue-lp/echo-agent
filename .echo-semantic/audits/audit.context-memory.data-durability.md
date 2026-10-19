---
schema_version: 1
id: audit.context-memory.data-durability
kind: audit
boundary_ref: boundary.context-memory
lens: data_durability
freshness: examined
revision: source:b2639597dfa94b7e16aa312e46db6585d9df7636d6340c004ae978f18711d0f1
finding_refs: [finding.transcript-projection-settlement, finding.transcript-generation-runtime-identity, finding.checkpoint-current-plan-orphan-authority, finding.compacted-transcript-retained-ordinals, finding.canonical-root-system-refresh]
challenges:
  compacted-retained-occurrence-identity:
    revision: source:b2639597dfa94b7e16aa312e46db6585d9df7636d6340c004ae978f18711d0f1
    source_refs: [src/agent/snapshot.rs, src/state/mod.rs, echo-state/src/compression/mod.rs, src/agent/react/capabilities.rs, src/agent/react/run/phases/compact.rs, src/agent/react/tests.rs]
    evidence_refs: [evidence.compacted-context-projection-repair]
  canonical-root-refresh-identity:
    revision: source:b2639597dfa94b7e16aa312e46db6585d9df7636d6340c004ae978f18711d0f1
    source_refs: [echo-state/src/compression/mod.rs, src/agent/react/mod.rs, src/agent/react/capabilities.rs, src/agent/react/run/context.rs, src/agent/react/tests.rs]
    evidence_refs: [evidence.compacted-context-projection-repair]
  transcript-projection-settlement:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/agent/snapshot.rs, src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/finalize.rs]
    evidence_refs: [evidence.agent-context-execution, evidence.persistence-observation]
  runtime-incarnation-clear:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/state/file.rs, src/state/sqlite.rs, src/agent/react/run/stream_channel.rs]
    evidence_refs: [evidence.persistence-observation]
  checkpoint-identity-and-plan:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/agent/snapshot.rs, src/state/mod.rs, src/agent/react/mod.rs]
    evidence_refs: [evidence.agent-context-execution]
---

# Context、Transcript 与 Checkpoint 数据持久性审计

The original sections below retain the f1e fault epoch. Only the two new
challenges bind current source:b2639597dfa94b7e16aa312e46db6585d9df7636d6340c004ae978f18711d0f1; examined means these hypotheses were
checked, not that the broad Behavior or future SDK/main/full gates are verified.

## 审查范围

审查 runtime checkpoint、ConversationStore transcript projection、runtime/transcript identity、incarnation clear 和 `AgentCheckpoint.current_plan`；长期 Evolution mutation 不在本单元。

## 已检查故障假设

验证 checkpoint 与 transcript partial success 是否丢失可恢复历史、不同 runtime/generation identity 是否能写出不可恢复状态、File/SQLite clear 是否跨 scope 混写，以及 current_plan 是否有 canonical writer。

## 实际实现路径与证据

Transcript ensure/load/project/save failure 只告警；pre-compact 路径仍保存 checkpoint、压缩并 realign cursor，finalize 也先提交 checkpoint 后 best-effort 保存 transcript。File owner record 原子替换，SQLite save/clear 使用事务，现有内建 backend 未显示 scope clear 反例。Invocation context 允许 runtime state ID 与 transcript generation ID 独立配置，但恢复要求相等。`current_plan` 只有 reset/restore/重写路径，未发现 canonical Task artifact writer。

## 问题记录

确认 `finding.transcript-projection-settlement`；新增 runtime/transcript identity 与 current_plan 孤立权威两个 Finding。ContextAssembler 是无状态 custom-loop building block，不因本持久性视角要求与默认 ContextManager 等价。

## 残余风险

跨 store 删除仍要求 caller 先关闭 admission 并结算 owner；现有 transcript tests 不覆盖 backend failure + compaction crash cut。

## 未检查项

未验证 EKO 或外部 consumer 是否满足 ADR 0006 admission barrier，未执行 ConversationStore 故障注入，未审查长期 Evolution mutation。

## Current retained-view and canonical-root challenges

Independent source/runtime rereview is C0/I0/M0 for this slice. The first defect
reallocated retained facts; sparse derived bindings now preserve original facts,
strict ordering and new-turn append identity. The second persisted the right
window but refreshed the wrong System occurrence; the shared canonical owner
now preserves independent System payloads and renders the authoritative
override-first current root value. Original before cuts, fixture-only
errors, 12 distinct executed focused cases plus same-case value strengthening,
current clippy/fmt, read-only Store
reentry, synchronous cancellation restoration and observer acceptance are bound
by evidence.compacted-context-projection-repair. Volatile restoration is not durable rollback; recovery reloads
actual state and does not replay promotion. Finding resolution is local focused
framework scope. Metadata/new ABI and new SDK/main delivery remain separate.
