---
schema_version: 1
id: audit.observation-persistence-delivery.data-durability
kind: audit
boundary_ref: boundary.observation-persistence-delivery
lens: data_durability
freshness: stale
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.trace-audit-secret-boundary, finding.checkpoint-journal-binding, finding.diagnostic-persistence-failure-visibility]
challenges:
  checkpoint-source-binding:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-state/src/journal/mod.rs, echo-state/src/journal/file.rs, echo-state/src/delivery.rs]
    evidence_refs: [evidence.persistence-observation]
  diagnostic-failure-visibility:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/trace/mod.rs, src/agent/react/mod.rs, src/agent/snapshot.rs, echo-state/src/audit/mod.rs, echo-state/src/audit/file.rs]
    evidence_refs: [evidence.persistence-observation, evidence.effects-extensions, evidence.diagnostic-persistence-failure-visibility-repair]
  journal-delivery-recovery:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-state/src/journal/file.rs, echo-state/src/journal/segmented.rs, echo-state/src/delivery.rs]
    evidence_refs: [evidence.persistence-observation]
---

# Observation 与 Delivery 数据持久性审计

## 审查范围

审查 EventJournal batch outcome、checkpoint binding/recovery、segmented retention、Delivery attempt identity、RunStore/AuditLogger failure visibility 和 secret retention。

## 已检查故障假设

验证合法但来自错误 Journal 的 checkpoint 是否被接受，诊断 backend 失败是否产生结构化信号，以及 unknown append/retention/Delivery chain 是否存在新反例。

## 实际实现路径与证据

原revision确认Checkpoint/Journal identity与diagnostic failure visibility缺口。当前未提交repair候选使RunStore missing append、trace start/append/load/finalize、Audit callback、observer liveness与FileAudit durable recovery进入新路径；该路径已通过独立静态反证，但工程验证和最终revision复审未完成，因此本Audit标记stale，不把候选结果写成examined事实。

## 问题记录

Checkpoint/journal binding与diagnostic failure visibility继续由独立Finding追踪；#46已有repair Evidence但保持open。secret retention、InMemory audit成功丢写与tool terminal分歧分别由#103、#61、#102追踪。

## 残余风险

物理 Journal prefix prune 后错误 checkpoint更无法重建；FileEventJournal cursor 位于缓存尾部时未重新校验 live file identity，保留 residual。

## 未检查项

未检查所有自定义 Store/Journal/AuditLogger、应用 UI/feed retention 或真实 crash/fsync 故障。
