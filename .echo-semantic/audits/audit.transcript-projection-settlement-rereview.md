---
schema_version: 1
id: audit.transcript-projection-settlement-rereview
kind: audit
boundary_ref: boundary.context-memory
lens: data_durability
freshness: examined
revision: source:0a91548f9c8d3f6e6a19bc2025fd2d21a46b656162f50b44aedfba5b6d5bf1bb
finding_refs: [finding.transcript-projection-settlement]
challenges:
  authority-and-attempt:
    revision: source:0a91548f9c8d3f6e6a19bc2025fd2d21a46b656162f50b44aedfba5b6d5bf1bb
    source_refs: [src/state/mod.rs, src/state/file.rs, src/state/sqlite.rs, src/agent/snapshot.rs]
    evidence_refs: [evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification]
  deadline-and-recovery:
    revision: source:0a91548f9c8d3f6e6a19bc2025fd2d21a46b656162f50b44aedfba5b6d5bf1bb
    source_refs: [echo-core/src/memory/conversation.rs, echo-state/src/memory/file_conversation.rs, echo-state/src/memory/sqlite_conversation.rs, src/state/mod.rs]
    evidence_refs: [evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification]
  terminal-and-observation:
    revision: source:0a91548f9c8d3f6e6a19bc2025fd2d21a46b656162f50b44aedfba5b6d5bf1bb
    source_refs: [src/agent/react/run/stream_channel.rs, src/agent/react/run/react_loop.rs, src/agent/react/run/phases/compact.rs, src/agent/react/run/phases/tools.rs, src/agent/react/run/phases/finalize.rs]
    evidence_refs: [evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification]
  clear-delete-recreate:
    revision: source:0a91548f9c8d3f6e6a19bc2025fd2d21a46b656162f50b44aedfba5b6d5bf1bb
    source_refs: [src/state/mod.rs, src/state/file.rs, src/state/sqlite.rs, docs/adr/0056-durable-transcript-projection-settlement.md]
    evidence_refs: [evidence.transcript-projection-settlement-repair, evidence.transcript-projection-settlement-verification]
---

# Transcript projection durable settlement independent rereview

## 审查范围

复审唯一状态权威、pending attempt、apply/proof-ack、absolute deadline、warm/cold recovery、terminal
ordering、typed observation、trace finalization、exact clear、managed delete、retention 与 recreate 隔离。

## 已检查故障假设

- 两个 Agent 或进程用 stale revision 覆盖、ack 或清除其它 writer 的 pending；
- apply/ack timeout 后 failure metadata 无法落盘，或 nonretryable result 被错误降为 Deferred；
- backend 排队、authority lock 或 SQLite transaction lock 越过 caller deadline 后仍开始写；
- guard、hook、provider/tool failure、cancel、consumer drop 或 max iteration 在 settlement 前发布终态；
- reconciliation 把其它 Agent checkpoint revision 错当成本实例 hydrated context；
- exact clear 或旧 delete replay 在 conversation recreate 后删除新 epoch；
- ReceiptExpired、corrupt manifest 或 legacy raw mutator 绕过 managed authority。

## 实际实现路径与证据

Conversation、Runtime backend 与 state helper 三个切片分别经过独立复审；集成 reviewer 从
`origin/main@99b9abd6` 对最终 framework 候选反复提出反例并在修复后锁定复审。最终结论为 PASS，
Critical 0、Important 0、Minor 0。Focused tests 与 typed contract evidence 见 repair/verification Evidence。

## 问题记录

复审中发现并关闭 revision conflict 复用旧 context、跨 Agent hydrated version、final intervention 与
Think 双终态、deadline reserve/authority-lock、attempt off-by-one、scope retirement admission、early trace
observation、max-iteration hook ordering、delete/clear lost-ack recreate 等反例。

## 残余风险

独立 consumer 尚未承接 public Store、deadline、receipt 与 settlement event 时，应在其所属仓库
保留采用 Finding；这不会重开 framework Finding，也不阻塞 GitHub Issue #106 关闭。

## 未检查项

原 repair reviewer 未直接运行远端 Linux/Windows CI；current main 的七项 CI 已由 closure Evidence
核实。应用 UI 如何展示 Deferred 属于 embedding product policy。

Framework-only closure 在最终 source digest 上复核 `3735f7e0` observer ordering、双语文档、
focused tests、完整门禁和 17-feature matrix，没有发现新的 framework blocker。
