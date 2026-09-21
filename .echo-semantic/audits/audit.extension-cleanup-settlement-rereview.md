---
schema_version: 1
id: audit.extension-cleanup-settlement-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: failure_concurrency
freshness: examined
revision: source:4887582b3c8c982732d721189145bdf28cbd3a06ce0881a785706fc514d3c6e7
finding_refs: [finding.extension-cleanup-settlement]
challenges:
  pending-and-post-settlement:
    revision: source:4887582b3c8c982732d721189145bdf28cbd3a06ce0881a785706fc514d3c6e7
    source_refs: [echo-integration/src/mcp/transport/mod.rs, echo-integration/src/mcp/transport/sse.rs, echo-integration/src/mcp/transport/stdio.rs]
    evidence_refs: [evidence.extension-cleanup-settlement-repair, evidence.extension-cleanup-settlement-verification]
  cancellation-resilient-close-owner:
    revision: source:4887582b3c8c982732d721189145bdf28cbd3a06ce0881a785706fc514d3c6e7
    source_refs: [echo-integration/src/mcp/transport/sse.rs, echo-integration/src/mcp/transport/stdio.rs]
    evidence_refs: [evidence.extension-cleanup-settlement-repair, evidence.extension-cleanup-settlement-verification]
  construction-cancellation-owner:
    revision: source:4887582b3c8c982732d721189145bdf28cbd3a06ce0881a785706fc514d3c6e7
    source_refs: [echo-integration/src/mcp/client.rs, echo-integration/src/mcp/transport/sse.rs]
    evidence_refs: [evidence.extension-cleanup-settlement-repair, evidence.extension-cleanup-settlement-verification]
  manager-debt-retry:
    revision: source:4887582b3c8c982732d721189145bdf28cbd3a06ce0881a785706fc514d3c6e7
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

Transport close 修复已引入 single-flight receipt、manager close gate、成功后移除/失败保留 debt，
并覆盖 stdio task abort、manager caller-cancel/并发 close 与显式 POST readiness。当前
framework-only 复审发现新的 blocker：`McpPreparationOwner::drop` 与 SSE construction Drop 仍派生
没有 JoinHandle/receipt 的后台清理，runtime shutdown 可以在结算前截断它。

## 问题记录

Transport close 子范围具备 repair、verification 与 rereview 证据；construction cancellation owner
仍有 Important finding，因此 `finding.extension-cleanup-settlement` 与 Issue #55 保持 open。原
discovery 中的 SDK/LSP 部分不再由本 framework Finding 持有。

## 残余风险

第三方 `McpTransport` 实现必须遵守新的 fallible close settlement 合同。进程级 runtime 强制退出
仍可截断未被 awaited owner 持有的 construction cleanup；这正是后续 repair 的当前风险。

## 未检查项

未连接真实第三方 MCP server；LSP、Plugin coordinator 与 Agent adapter close 属独立 Finding。
