---
schema_version: 1
id: audit.extension-cleanup-settlement-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: failure_concurrency
freshness: examined
revision: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
finding_refs: [finding.extension-cleanup-settlement]
challenges:
  pending-and-post-settlement:
    revision: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
    source_refs: [echo-integration/src/mcp/transport/mod.rs, echo-integration/src/mcp/transport/sse.rs, echo-integration/src/mcp/transport/stdio.rs]
    evidence_refs: [evidence.extension-cleanup-settlement-repair, evidence.extension-cleanup-settlement-verification]
  cancellation-resilient-close-owner:
    revision: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
    source_refs: [echo-integration/src/mcp/transport/sse.rs, echo-integration/src/mcp/transport/stdio.rs]
    evidence_refs: [evidence.extension-cleanup-settlement-repair, evidence.extension-cleanup-settlement-verification]
  construction-cancellation-owner:
    revision: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
    source_refs: [echo-integration/src/mcp/client.rs, echo-integration/src/mcp/transport/sse.rs]
    evidence_refs: [evidence.extension-cleanup-settlement-repair, evidence.extension-cleanup-settlement-verification]
  manager-debt-retry:
    revision: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
    source_refs: [echo-integration/src/mcp/mod.rs]
    evidence_refs: [evidence.extension-cleanup-settlement-repair, evidence.extension-cleanup-settlement-verification]
---

# MCP transport cleanup settlement 独立复审

## 审查范围

复审 MCP pending request、SSE receive/request/notification POST、stdio stdin/stdout/stderr/child、
transport/client/manager close、preparation/construction cancellation、replacement/disconnect/close_all、
Agent/Plugin adapter 错误传播、调用 Future cancellation 与并发 close。

## 已检查故障假设

检查 endpoint/POST/write/flush/timeout/channel/caller-drop 是否残留 pending；close 是否早于 I/O
task、writer 或 child；stdout EOF/read-error 是否留下 process；close 与 construction Future 被取消
是否 drop 唯一 owner；并发 close 是否假成功；manager 是否在 fallible close 前删除 owner；
replacement 是否形成返回失败但新 target 已发布；notification POST 是否越过 close settlement boundary。

## 实际实现路径与证据

Transport close 修复此前已引入 single-flight receipt、manager close gate、成功后移除/失败
保留 debt。原 framework-only 复审发现 preparation 与 SSE construction Drop 派生不可等待清理。
本轮独立只读 reviewer 沿 preparation owner、caller cancellation、runtime-drop retry、
double-close、独占 topology 和错误分类复审当前实现，结论 PASS、0 findings。manager 在首个
资源创建 poll 前登记 preparation scope；`close_all` 等待或保留失败 scope；SSE receive task
在 construction 暂停前已进入 transport owner，stdio child 和 close receipt 在中断后仍可重试。

## 问题记录

原 `main@e15cc17f` 的 construction cancellation blocker 在本任务分支 framework 范围内闭合。
Finding 当前 lane-local resolved；本地完整门禁与条件矩阵已通过，Issue #55 仍 OPEN，等待 PR/CI 和远端 main
交付。原 discovery 中的 SDK Host/LSP 部分不由本 Finding 持有。

## 残余风险

第三方 `McpTransport` 实现必须遵守 fallible close settlement 合同。直接 consumer 丢弃
preparation waiter 时必须保留 cleanup scope 并等待关闭；进程级强制退出仍可能截断未完成结算。

## 未检查项

未连接真实第三方 MCP server；独立 reviewer 未自行运行测试。LSP、Plugin coordinator 与
Agent adapter close 属独立 Finding。
