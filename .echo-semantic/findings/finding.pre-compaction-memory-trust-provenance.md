---
schema_version: 1
id: finding.pre-compaction-memory-trust-provenance
kind: finding
type: authority_conflict
status: resolved
severity: high
primary_focus: permission_external
focus: [data_durability, trigger_input, state_authority]
boundary_ref: boundary.eval-evolution
behavior_refs: [behavior.eval-evolution, behavior.context-memory-lifecycle]
rule_refs: [rule.quality-observation-boundary, rule.context-persistence-separation]
evidence_refs: [evidence.provider-protocol-quality, evidence.agent-context-execution, evidence.memory-provenance-authority-repair, evidence.memory-provenance-authority-verification]
audit_refs: [audit.eval-evolution.permission-external]
decision_refs: [decision-adr-0070-memory-provenance-and-recall-authority]
repair_evidence_refs: [evidence.memory-provenance-authority-repair]
verification_evidence_refs: [evidence.memory-provenance-authority-verification]
rereview_audit_refs: [audit.memory-provenance-authority-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Pre-compaction memory丢失混合来源trust provenance

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/76

## 问题

Pre-compaction flush从user/assistant/tool混合transcript经LLM生成内容，无exact-evidence/origin验证，统一标L3Promotion+Active写入warm memory；Recall只排除Superseded。

## 触发条件与影响

工具输出或assistant推断可被提升为跨轮次可召回Active memory，来源可信度和批准语义丢失并影响后续prompt。

## 证据

`src/agent/react/run/phases/compact.rs`、`run/context.rs`与`src/evolution/recall.rs`展示自动写入和召回路径。

## 处理记录

ADR 0070 将来源角色、精确引用、信任与显式批准分离。自动 writer 只保存 Draft，
MemoryLayerManager 按 exact snapshot 和 journal generation 激活；自动、Store 工具与分层
recall 只消费已批准 Active/Archived，Hot 晋升仍保留自动上下文可见性。
合成 runtime/Horizon 消息不作为用户原文证据。focused File/SQLite、取消、失败重启、
ABA 与旧数据回归已通过。完整门禁、17-feature matrix 与独立 rereview PASS。
PR #152 七项 CI 全绿并 squash merge 到已签名的 framework
`main@5a0f2af2da8de9db2bf98c3aa8dd2a54e1152d7c`；合并后 strict semantic
在相同 source digest 上通过。SDK、CLI、website 与 A2A 不参与本 Finding 判定。
