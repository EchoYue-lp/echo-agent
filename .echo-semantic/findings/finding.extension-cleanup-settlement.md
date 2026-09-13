---
schema_version: 1
id: finding.extension-cleanup-settlement
kind: finding
type: implementation_bug
status: open
severity: high
primary_focus: failure_concurrency
focus: [time_lifecycle, result_side_effect, state_authority]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions]
audit_refs: []
decision_refs: []
repair_evidence_refs: []
verification_evidence_refs: []
rereview_audit_refs: []
discovered_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
---

# MCP SSE 与 SDK LSP cleanup 未等待结算

## 问题

SSE endpoint/POST/timeout 失败可遗留 pending sender，close 只 cancel 不 drain/await；SDK Host LSP close 清 Arc map 而未调用 manager shutdown_all。

## 触发条件与影响

Transport 建立失败、连接关闭或 Host shutdown 时，pending request/child process 可能在终态后继续存在或只依赖 Drop。

## 证据

`echo-integration/src/mcp/transport/sse.rs` 与 `echo-sdk-host/src/core_profile/facade/integrations.rs` 显示当前 close 路径。

## 处理记录

Discovery 记录；下一阶段用 pending request 与 real child shutdown 测试验证 cleanup deadline。
