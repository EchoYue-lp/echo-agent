---
schema_version: 1
id: finding.subagent-factory-publication-race
kind: finding
type: implementation_bug
status: resolved
severity: high
primary_focus: failure_concurrency
focus: [state_authority, time_lifecycle, contract_evidence]
boundary_ref: boundary.task-subagent-workflow
behavior_refs: [behavior.task-subagent-execution]
rule_refs: [rule.task-subagent-authority]
evidence_refs: [evidence.task-subagent-workflow, evidence.subagent-factory-singleflight-repair, evidence.subagent-factory-singleflight-verification]
audit_refs: [audit.task-subagent-workflow.state-authority, audit.subagent-factory-singleflight-rereview]
decision_refs: []
repair_evidence_refs: [evidence.subagent-factory-singleflight-repair]
verification_evidence_refs: [evidence.subagent-factory-singleflight-verification]
rereview_audit_refs: [audit.subagent-factory-singleflight-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Subagent lazy factory publication 存在双创建窗口

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/25

## 问题

基准实现中factory完成后先从`instantiating`删除名称，再取得state lock发布agent；test-only publication boundary确定性确认第二调用可在空窗中启动同revision第二次factory。

## 触发条件与影响

并发 resolve 恰好落在 publication gap 时，同一 Subagent definition 可创建两个实例并竞争发布，违反防 double-creation 的 registry authority。

## 证据

当前成功值在per-entry OnceCell中原子发布，第二resolver在第一resolver返回前即可读取同一Arc；revision/cell identity阻止旧代结果进入当前entry。

## 处理记录

取消与发布竞态已由同一single-flight authority原子修复；正信号交错测试与独立复审排除了未调度导致的假阳性，本Finding已关闭。
