---
schema_version: 1
id: audit.plugin-mcp-owner-isolation-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: state_authority
freshness: examined
revision: c327f2d0f030184cd2e28f9d82e6c6930d9098bb
finding_refs: [finding.plugin-mcp-owner-isolation]
challenges:
  typed-owner-lifecycle-authority:
    revision: c327f2d0f030184cd2e28f9d82e6c6930d9098bb
    source_refs: [echo-integration/src/mcp/identity.rs, echo-integration/src/mcp/mod.rs]
    evidence_refs: [evidence.plugin-mcp-owner-isolation-repair, evidence.plugin-mcp-owner-isolation-verification]
  tool-resource-hook-projection-settlement:
    revision: c327f2d0f030184cd2e28f9d82e6c6930d9098bb
    source_refs: [echo-integration/src/mcp/resource_tool.rs, echo-integration/src/mcp/tool_adapter.rs, src/agent/react/capabilities.rs, src/agent/react/mod.rs]
    evidence_refs: [evidence.plugin-mcp-owner-isolation-repair, evidence.plugin-mcp-owner-isolation-verification]
  plugin-receipt-and-direct-api-compatibility:
    revision: c327f2d0f030184cd2e28f9d82e6c6930d9098bb
    source_refs: [src/plugin/prepared.rs, src/mcp.rs, tests/facade_smoke.rs, docs/adr/0067-mcp-owner-qualified-identity.md]
    evidence_refs: [evidence.plugin-mcp-owner-isolation-repair, evidence.plugin-mcp-owner-isolation-verification]
---

# Plugin MCP owner isolation independent rereview

## 审查范围

复审 MCP typed owner identity、active/prepared/cleanup/closing authority、同名 Direct 与多个
Plugin 的共存和精确撤销、replacement/disconnect/close_all 失败结算、Tool/Resource/Hook
投影、selector 可逆性、Plugin receipt 完整性与旧 Direct API source compatibility。

## 已检查故障假设

- Plugin cleanup retry 被降级成 Direct identity，或旧 owner 撤销另一个同名 owner；
- replacement/disconnect close 失败后 Agent 保留已撤销 target 的 Tool/Resource/Hook；
- close_all 使用展示字符串删除错误投影；
- Plugin Hook 以裸 local name 路由到错误 client；
- Direct 保留前缀、Unicode、标点或同 URI 资源发生 selector/投影别名；
- typed receipt inventory 被篡改后仍触发 cleanup；
- typed resource builder 破坏既有 Direct public API。

## 实际实现路径与证据

`McpServerId { owner, local_name }` 是 manager 全部生命周期状态与 debt 的 key，旧字符串 API
严格映射 Direct。Plugin wiring 从不可变 `PreparedPlugin.id` 注入 owner，receipt equality
覆盖 typed IDs。Resource directory 直接保存 selector 到 typed identity/client 的映射；Tool
有损投影携带结构边界摘要，Hook 注册前绑定 Plugin owner。失败 settlement 以真实 manager
topology 同步刷新 Agent 投影，同时保留 cleanup error/debt。

独立复审经过四轮：首轮发现 6 个 Important 与 1 个 Minor；二轮确认 6 个 Important 已闭合，
同时确认原 Minor 仍在并新增 public builder source compatibility Important；三轮确认这两项
已修复；最终 advancing-base 复审为 0 Critical、0 Important、0 Minor。最终分支 `fb231667`
与远端 squash commit `c327f2d0` 的 tree hash
均为 `4ac6d3700b21ed4302160e6f39d3a51feac83143`。PR #141 七项 CI 全绿，GitHub commit
verification 为 valid。

## 问题记录

未发现剩余 Critical、Important 或 Minor 问题；
`finding.plugin-mcp-owner-isolation` 具备 repair、verification、independent rereview 与
remote-main delivery 证据，可以标记 resolved。

## 残余风险

Registry 持久状态、component wiring 与 callback lifecycle 的跨组件统一事务继续由 #73
追踪。测试使用 in-process transport 与本地 stdio fixture，未建立真实远端 MCP 互操作证明。
进程强制退出仍可能截断异步 cleanup，但不会把一个 owner 的 debt 绑定到另一个 owner。

## 未检查项

未执行真实远端 MCP server、embedding application reload transaction、跨进程恢复或 #73
host coordinator 实现。
