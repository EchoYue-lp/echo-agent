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
    evidence_refs: [evidence.persistence-observation, evidence.effects-extensions]
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

CheckpointFrame 只有 sequence/state，文件摘要不绑定 Journal identity；recover 只检查序号范围，因此 Journal B 的同序号合法 checkpoint 可被 Journal A 接受。RunStore 默认 append 对缺失 run 返回成功，trace start save 失败仍返回 run ID，多条 trace/audit callback 丢弃 backend error；FileAuditLogger 只有 flush。File Journal ambiguous append、segmented retained floor 与 Delivery attempt/turn 校验在已审路径未发现新反例。

## 问题记录

新增 checkpoint/journal binding 与 diagnostic failure visibility 两个 Finding；secret retention Finding 保持 open 且与失败可见性分离。

Checkpoint/Journal identity修复候选已改变本Audit检查过的源码与持久格式；focused测试和独立复审完成前，本Audit保持stale，原examined结论不得用于关闭Finding。

## 残余风险

物理 Journal prefix prune 后错误 checkpoint更无法重建；FileEventJournal cursor 位于缓存尾部时未重新校验 live file identity，保留 residual。

## 未检查项

未检查所有自定义 Store/Journal/AuditLogger、应用 UI/feed retention 或真实 crash/fsync 故障。
