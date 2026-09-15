---
schema_version: 1
id: audit.nonstream-cancellation-parity-rereview
kind: audit
boundary_ref: boundary.llm-provider-runtime
lens: failure_concurrency
freshness: examined
revision: 13a6a74222249dfd370a15f7d8b23c9f9a4a9553
finding_refs: [finding.nonstream-cancellation-parity]
challenges:
  full-request-cancellation-boundary:
    revision: 13a6a74222249dfd370a15f7d8b23c9f9a4a9553
    source_refs: [echo-integration/src/providers/client.rs, echo-integration/src/providers/openai.rs, echo-integration/src/providers/responses.rs, echo-integration/src/providers/anthropic.rs]
    evidence_refs: [evidence.nonstream-cancellation-parity-repair, evidence.nonstream-cancellation-parity-verification]
  strict-decode-and-task-settlement:
    revision: 13a6a74222249dfd370a15f7d8b23c9f9a4a9553
    source_refs: [echo-integration/src/providers/client.rs]
    evidence_refs: [evidence.nonstream-cancellation-parity-verification]
---

# Non-stream provider cancellation独立复审

## 审查范围

独立reviewer检查统一transport、三provider接线、同步decode取消、blocking task结算、
非法UTF-8与日志secret边界，并排除其它并行diff。

## 已检查故障假设

检查headers前后取消、body读取与取消竞态、同步decode不轮询、blocking task泄漏、
provider projection后取消、lossy UTF-8成功以及raw body日志泄漏。

## 实际实现路径与证据

首次review发现decode取消和lossy UTF-8两个Important；修复后最终review确认所有路径通过
shared helper，decode cooperative stop并await，三provider在projection后复核token，日志
只记录raw length。结论pass，Critical、Important、Minor均为0。

## 问题记录

首次review的两个Important均已由commit `13a6a74222249dfd370a15f7d8b23c9f9a4a9553`
修复并加入确定性反例。

## 残余风险

真实provider可能有协议专属stream terminal；该风险属于#78，不影响non-stream取消合同。

## 未检查项

未运行真实公网provider、完整workspace门禁或远端CI。
