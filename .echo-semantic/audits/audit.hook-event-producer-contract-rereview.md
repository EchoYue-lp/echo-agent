---
schema_version: 1
id: audit.hook-event-producer-contract-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: contract_evidence
freshness: examined
revision: 1465a37101a6cc30f7e884ab3c138353aa8f1cda
finding_refs: [finding.hook-event-producer-contract]
challenges:
  exhaustive-matrix:
    revision: 1465a37101a6cc30f7e884ab3c138353aa8f1cda
    source_refs: [echo-core/src/hooks/types.rs, docs/en/23-hooks.md, docs/zh/23-hooks.md, tests/hook_event_producer_contract.rs]
    evidence_refs: [evidence.hook-event-producer-contract-repair, evidence.hook-event-producer-contract-verification]
---

# HookEvent producer contract rereview

## 审查范围

复核 31 个 HookEvent 的双语 producer matrix、ADR 分类、exhaustive contract test
以及 PermissionDenied、Task、Evolution、Subagent ownership 边界。

## 已检查故障假设

- catalog membership 被误当作自动 producer；
- generic lifecycle helper 被误当作事实源；
- host-owned bridge 与 framework executor 被重复接线；
- PermissionDenied 被提前接入第二个 permission authority。

## 实际实现路径与证据

矩阵、ADR 与 contract test 一致，测试和文档校验通过；未新增 runtime producer
或第二状态权威。

## 残余风险

PermissionDenied 的 exactly-once producer 仍由 #37 负责；host adapters 依赖应用接线。

## 问题记录

独立复核未发现 framework blocker。

## 未检查项

未执行 full workspace gate、远端 CI 或 embedding consumer 验证。
