---
schema_version: 1
id: audit.mcp-tool-local-classification-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: permission_external
freshness: examined
revision: 72d1fccf74b85afe9a74e684ca3748b64642affb
finding_refs: [finding.mcp-tool-permission-classification]
challenges:
  annotation-trust-boundary:
    revision: 72d1fccf74b85afe9a74e684ca3748b64642affb
    source_refs: [echo-integration/src/mcp/tool_adapter.rs]
    evidence_refs: [evidence.mcp-tool-local-classification-repair, evidence.mcp-tool-local-classification-verification]
  plan-surface-and-execution:
    revision: 72d1fccf74b85afe9a74e684ca3748b64642affb
    source_refs: [src/agent/snapshot.rs, src/agent/react/run/pipeline.rs]
    evidence_refs: [evidence.mcp-tool-local-classification-repair, evidence.mcp-tool-local-classification-verification]
  live-permission-mode-authority:
    revision: 72d1fccf74b85afe9a74e684ca3748b64642affb
    source_refs: [echo-orchestration/src/human_loop/service.rs, src/agent/snapshot.rs]
    evidence_refs: [evidence.mcp-tool-local-classification-verification]
---

# MCP Tool 本地分类独立复审

## 审查范围

复审 MCP annotation trust boundary、本地 ToolCapabilities、permission/Plan mode surface、执行 hard
gate、live PermissionService mode 与失败 side-effect settlement。

## 已检查故障假设

1. 伪造 readOnlyHint 是否能降低默认 MCP capability 或失败 side effect。
2. MCP-qualified mutating tool 是否仍因名称过滤而进入 Plan surface 或执行。
3. SDK/host 直接修改 PermissionService mode 是否绕过 AgentConfig snapshot。
4. Permission hook Allow 是否能在 Plan hard gate 之前放行 mutating MCP tool。
5. 用户主动 connect/reconcile/disconnect/close 是否被错误加入 Agent permission gate。

## 实际实现路径与证据

Annotations 只保留在协议 McpTool 元数据中；adapter 默认分类和显式本地覆盖均由一个
ToolCapabilities 快照承担。ToolRuntime 和 PlanModeStage 使用 ToolAccess；PermissionService 的原子
current mode 由全部 mode 写入口维护，并被既有 snapshot 实时观察。PlanModeStage 在 PreToolUse 与
Permission hook 之前执行。分层 adapter 测试和完整 pipeline 反例共同证明两类入口均未执行 tool。

## 问题记录

最终独立复审在 `72d1fccf74b85afe9a74e684ca3748b64642affb` 上返回 pass，Critical、
Important、Minor 均为 0；Finding #66 具备 repair、verification 与 rereview 证据，可标记 resolved。

## 残余风险

最终多分支整合尚需统一重生成 public inventory 并执行完整 MR gate；该交付门禁不改变本次已验证
的 annotation、Plan 和 side-effect 行为。

## 未检查项

未执行第三方生产 MCP server 互操作、长时间并发 stress、Windows transport 或最终远端 CI。
