---
schema_version: 1
id: audit.extension-cleanup-settlement-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: failure_concurrency
freshness: examined
revision: source:1793556b87f7275872eba5723d9643e2ec78e14e5e11cbdfc5c3f7a3c1a3ce70
finding_refs: [finding.extension-cleanup-settlement]
challenges:
  pending-and-post-settlement:
    revision: source:1793556b87f7275872eba5723d9643e2ec78e14e5e11cbdfc5c3f7a3c1a3ce70
    source_refs: [echo-integration/src/mcp/transport/mod.rs, echo-integration/src/mcp/transport/sse.rs, echo-integration/src/mcp/transport/stdio.rs]
    evidence_refs: [evidence.extension-cleanup-settlement-repair, evidence.extension-cleanup-settlement-verification]
  cancellation-resilient-close-owner:
    revision: source:1793556b87f7275872eba5723d9643e2ec78e14e5e11cbdfc5c3f7a3c1a3ce70
    source_refs: [echo-integration/src/mcp/transport/sse.rs, echo-integration/src/mcp/transport/stdio.rs]
    evidence_refs: [evidence.extension-cleanup-settlement-repair, evidence.extension-cleanup-settlement-verification]
  manager-and-sdk-debt-retry:
    revision: source:1793556b87f7275872eba5723d9643e2ec78e14e5e11cbdfc5c3f7a3c1a3ce70
    source_refs: [echo-integration/src/mcp/mod.rs]
    evidence_refs: [evidence.extension-cleanup-settlement-repair, evidence.extension-cleanup-settlement-verification]
---

# MCP transport cleanup settlement 独立复审

## 审查范围

复审 MCP pending request、SSE receive/request/notification POST、stdio stdin/stdout/stderr/child、transport/client/manager close、replacement/disconnect/close_all、Agent/Plugin/SDK adapter 错误传播、调用 Future cancellation 与并发 close。

## 已检查故障假设

检查 endpoint/POST/write/flush/timeout/channel/caller-drop 是否残留 pending；close 是否早于 I/O task、writer 或 child；stdout EOF/read-error 是否留下 process；close Future 被取消是否 drop 唯一 owner；并发 close 是否假成功；manager/SDK 是否在 fallible close 前删除 owner；replacement 是否形成返回失败但新 target 已发布；notification POST 是否越过 close settlement boundary。

## 实际实现路径与证据

首轮复审发现 SSE notify、上层 Result、manager debt 与 close Future owner 四项 blocker；二轮补充发现 stdio task abort、manager caller-cancel/并发 close 与显式 POST readiness 测试。最终修复引入 transport single-flight receipt、manager close gate、成功后移除/失败保留 debt。最终独立 reviewer 对当前 source digest 返回 pass，确认 framework blocker 关闭且未增加权限门控。

## 问题记录

最终复审无 Critical、Important 或 Minor finding；`finding.extension-cleanup-settlement` 的 MCP transport 范围具备 repair、verification 与 rereview 证据。原 discovery 中的 LSP 部分已拆分到独立 LSP Findings，不由本 Finding 重复持有。

## 残余风险

第三方 `McpTransport` 实现必须遵守新的 fallible close settlement 合同；进程级 runtime 强制退出仍可截断所有异步 cleanup。上层 SDK adapter 由独立仓库负责，不由本 framework Finding 持有。

## 未检查项

未运行完整 workspace 合并门禁、逐 feature matrix、外部 MCP server acceptance 或远端 CI；未复审 LSP、Plugin coordinator 与 Agent adapter close 的独立 framework Finding。
