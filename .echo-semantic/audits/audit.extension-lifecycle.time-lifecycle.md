---
schema_version: 1
id: audit.extension-lifecycle.time-lifecycle
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: time_lifecycle
freshness: examined
revision: f1e9027246760661144786e9e35615cd46d580c6
finding_refs: [finding.plugin-lifecycle-coordination, finding.plugin-lifecycle-reconcile-overlap, finding.lsp-runtime-state, finding.lsp-manager-derived-handle-resurrection, finding.extension-cleanup-settlement, finding.mcp-client-capability-advertisement]
challenges:
  plugin-reconcile-and-withdraw:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-core/src/plugin/registry.rs, echo-core/src/plugin/lifecycle.rs, src/plugin/prepared.rs, echo-sdk-host/src/core_profile/facade/source_operations.rs]
    evidence_refs: [evidence.effects-extensions]
  mcp-session-close:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-integration/src/mcp/client.rs, echo-integration/src/mcp/transport/mod.rs, echo-integration/src/mcp/transport/sse.rs, echo-integration/src/mcp/transport/stdio.rs]
    evidence_refs: [evidence.effects-extensions]
  lsp-manager-and-derived-client:
    revision: f1e9027246760661144786e9e35615cd46d580c6
    source_refs: [echo-integration/src/lsp/client.rs, echo-integration/src/lsp/manager.rs, echo-sdk-host/src/core_profile/facade/integrations.rs, echo-sdk-host/src/core_profile/facade/mod.rs]
    evidence_refs: [evidence.effects-extensions]
---

# Plugin、MCP 与 LSP 时间生命周期审计

## 审查范围

审查 Plugin disable/uninstall/reconcile/unwire/callback、MCP SSE pending/reconnect/close/capability request，以及 LSP EOF/restart/SDK manager-client close。

## 已检查故障假设

验证撤销是否同步所有 owner、旧 deactivate 失败是否仍激活新代、SSE pending 是否 drain、LSP 状态是否反映 EOF，派生 handle 是否可在 manager close 后复活进程。

## 实际实现路径与证据

SDK disable/uninstall 只改 Registry，不联动 deactivate/unwire/event。Lifecycle reconcile 在旧 deactivate 失败后仍激活新集合。SSE endpoint/POST/timeout/reconnect failure 可遗留 pending，close 只 cancel 不 join/drain。MCP 广告反向 request capability 但 transport 无 handler。LSP EOF 只清 pending，restart/error 状态不更新；连接级 close_all 只清 manager maps。派生 lsp.client handle 无 parent fence，manager close 后仍可 initialize 新进程。

## 问题记录

确认四个既有 Finding；新增 reconcile overlap 与 derived-handle resurrection。派生 handle 的级联关闭/独立存活合同需 semantic-decide。

## 残余风险

局部单测通过但没有完整撤销链、SSE fault、LSP EOF/restart 或 manager/client generation 测试。

## 未检查项

未执行真实 SSE/LSP fault injection，未审查 EKO host 或 HTTP transport 的完整清理。
