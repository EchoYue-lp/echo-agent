---
schema_version: 1
id: finding.checkpoint-journal-binding
kind: finding
type: authority_conflict
status: open
severity: high
primary_focus: data_durability
focus: [state_authority, failure_concurrency, contract_evidence]
boundary_ref: boundary.observation-persistence-delivery
behavior_refs: [behavior.observation-persistence]
rule_refs: [rule.fact-projection-separation]
evidence_refs: [evidence.persistence-observation, evidence.checkpoint-journal-binding-repair, evidence.checkpoint-journal-binding-verification]
audit_refs: [audit.observation-persistence-delivery.data-durability, audit.checkpoint-journal-binding-rereview]
decision_refs: []
repair_evidence_refs: [evidence.checkpoint-journal-binding-repair]
verification_evidence_refs: [evidence.checkpoint-journal-binding-verification]
rereview_audit_refs: [audit.checkpoint-journal-binding-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Checkpoint 未绑定来源 Journal identity

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/43

## 问题

CheckpointFrame 与文件摘要只包含 sequence/state，recover 只检查序号范围；来自 Journal B 的同序号合法 checkpoint 可被 Journal A 接受。

## 触发条件与影响

配置、文件移动或 scope mix-up 把 checkpoint 与错误 Journal 配对时，系统返回 Loaded 并暴露另一事实流的 projection；prefix prune 后更无法重建。

## 证据

`echo-state/src/journal/mod.rs`、`journal/file.rs` 与 `delivery.rs` 展示 checkpoint schema、digest 和 recover/validate 边界。

## 处理记录

Data-durability Audit 确认。当前源码候选已把 checkpoint 绑定到 Journal generation identity，并加入同序号异源、异源receipt、同路径换代、schema v1、跨segment mix与prefix prune反例；独立源码和增量测试构造复审已pass，116项Journal focused tests、direct checks、两档Clippy与fmt全部通过。本 Finding 仍保持open，等待integration final gate和交付分支全局语义归并。
