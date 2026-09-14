---
schema_version: 1
id: audit.observation-persistence-delivery.contract-evidence
kind: audit
boundary_ref: boundary.observation-persistence-delivery
lens: contract_evidence
freshness: examined
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.trace-audit-secret-boundary, finding.trace-effect-event-producers, finding.tool-pipeline-example-drift, finding.workflow-entry-loop-drift, finding.hook-event-producer-contract, finding.in-memory-audit-successful-drop]
challenges:
  trace-and-audit-contract:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/agent/react/run/stream_channel.rs, src/agent/react/mod.rs, src/agent/react/run/pipeline.rs, src/trace/mod.rs, echo-state/src/audit/memory.rs, echo-state/src/audit/file.rs]
    evidence_refs: [evidence.persistence-observation, evidence.effects-extensions]
  event-producer-matrix:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/trace/mod.rs, echo-core/src/hooks/types.rs, echo-orchestration/src/workflow/mod.rs, docs/en/27-tracing.md]
    evidence_refs: [evidence.persistence-observation]
  executable-pipeline-contract:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [src/agent/react/run/pipeline.rs, echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs]
    evidence_refs: [evidence.effects-extensions, evidence.workspace-structure]
---

# Observation Event 与示例合同证据审计

## 审查范围

审查 Trace/Audit backend redaction、RunEvent/WorkflowEvent/HookEvent producer、消费者与文档，以及 demo64 对 production tool pipeline 的绑定。

## 已检查故障假设

验证 secret retention 是否超出 backend 证据，公开 event 变体是否没有自动 producer，示例是否静态复制而不检测生产顺序。

## 实际实现路径与证据

Trace 保存 guard 后的 effective input，但 in-memory RunStore/AuditLogger 原样且无统一容量/清洗；JSONL/File backend 才执行 sanitizer。RunEvent 的 PermissionDecision/FileEdit/TestRun/Error/SubagentRun 无生产点，文档仍宣称并写成 11 类而源码有 14 类。Workflow NodeError/Token 无内建 producer；多个 Hook lifecycle 只有通用手动 dispatch，PermissionDenied 无 dedicated producer。demo64 静态声明 13 stages，而 production default_pipeline 是 16 stages。

## 问题记录

确认并收窄 secret Finding，扩大 trace producer Finding，修正 demo64 Finding 的生产计数；新增 Hook producer contract 与 in-memory audit silent drop。

## 残余风险

缺 producer 可能是未实现能力，也可能是应删除的公共变体/文档承诺；进入各自 repair/decision 前不得假设。

## 未检查项

未审查应用 GUI/feed、全部自定义 backend 或 Hook 外部 effect 的二次 retention。
