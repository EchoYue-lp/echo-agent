---
schema_version: 1
id: audit.trace-audit-retention-contract-rereview
kind: audit
boundary_ref: boundary.observation-persistence-delivery
lens: data_durability
freshness: examined
revision: source:e09ea140ba27724c9d3fa92c16a8139087ddfd0256c1f0d614f153e6086b54cd
finding_refs: [finding.trace-audit-secret-boundary]
challenges:
  custom-audit-backend:
    revision: source:e09ea140ba27724c9d3fa92c16a8139087ddfd0256c1f0d614f153e6086b54cd
    source_refs: [echo-core/src/audit.rs, echo-state/src/audit/mod.rs, echo-state/src/audit/memory.rs, echo-state/src/audit/file.rs]
    evidence_refs: [evidence.trace-audit-retention-current-repair, evidence.trace-audit-retention-current-verification]
  identity-vs-content:
    revision: source:e09ea140ba27724c9d3fa92c16a8139087ddfd0256c1f0d614f153e6086b54cd
    source_refs: [src/trace/mod.rs, docs/adr/0074-trace-audit-retention-contract.md]
    evidence_refs: [evidence.trace-audit-retention-current-repair]
---

# Trace/audit retention contract rereview

The integrated snapshot preserves diagnostic identity fields for addressing while
requiring content retention at producer and custom backend boundaries. Partial
writes and finalization failures remain observable, and custom AuditLogger and
RunStore implementations are explicitly responsible for the same contract.

The focused trace/audit suites and independent rereview found no new framework
blocker. Secret recognition remains bounded by the configured content policy;
typed IDs and paths are not claimed to be globally redacted.

## 审查范围

审查整合快照中的 Trace、AuditLogger、AuditCallback、内存/文件 backend retention
边界、部分写入失败可见性和 typed diagnostic identity 保留语义。

## 已检查故障假设

- custom AuditLogger 绕过内容清洗；
- 部分写入失败被当作成功；
- 清洗 typed ID/path 破坏诊断寻址；
- RunStore 与 AuditLogger 合同不一致。

## 实际实现路径与证据

生产者先应用 retention，custom backend 合同要求再次应用并报告 Err；focused
trace/audit tests 覆盖失败状态和内容清洗，ADR 0074 记录字段分类。

## 残余风险

模式扫描不保证识别所有 credential 格式；background abort settlement 属于 #38/#61。

## 问题记录

本次复审未发现 framework blocker。

## 未检查项

未验证外部自定义 backend 的部署实现和跨仓库 consumer。
