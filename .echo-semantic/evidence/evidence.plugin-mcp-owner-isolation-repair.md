---
schema_version: 1
id: evidence.plugin-mcp-owner-isolation-repair
kind: evidence
observed_at: source:1793556b87f7275872eba5723d9643e2ec78e14e5e11cbdfc5c3f7a3c1a3ce70
source_refs:
  - echo-integration/src/mcp/identity.rs
  - echo-integration/src/mcp/mod.rs
  - echo-integration/src/mcp/resource_tool.rs
  - echo-integration/src/mcp/tool_adapter.rs
  - src/agent/react/capabilities.rs
  - src/agent/react/mod.rs
  - src/plugin/prepared.rs
  - docs/adr/0067-mcp-owner-qualified-identity.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - Plugin Registry persistence and full host reload coordination remain outside this identity repair (#73)
  - Tests do not establish real remote MCP interoperability
---

# Plugin MCP owner-qualified identity repair

## 支持的结论

`McpManager` 的 client、config、prepared、cleanup debt 与 closing debt 现在统一以结构化
`McpServerId { owner, local_name }` 为权威 key。旧字符串 API 只映射 Direct owner；Plugin
integration 从不可变 `PreparedPlugin.id` 注入 Plugin owner，receipt 同时保存类型化 identity。
公开 `build_mcp_resource_tools(HashMap<String, ...>)` 保留为 Direct compatibility wrapper，
typed owner 路径使用独立 `build_mcp_resource_tools_by_id`，避免破坏既有源码调用。
同名 Direct 与多个 Plugin server 因此可并存，disconnect、retry、replacement、cancelled
prepared cleanup 与 close_all 只结算精确 owner。
Receipt equality 同时覆盖 typed `mcp_connected_ids`，篡改 owner identity 的 receipt 会在
任何 cleanup side effect 前返回 `InvalidReceipt`。

Agent 在 owner target 被 replacement/disconnect 的失败 close 撤销后，同步撤销该 target 的
Tool、Resource 与 Hook MCP executor 投影；未被撤销的其它 owner 保持可达。Plugin Hook 中的
裸 MCP server 名在注册前按 prepared owner 转换成 selector，Hook executor 不再通过裸名选择
Plugin client。

Resource directory 保存 selector 到 typed identity/client 的直接映射，不从 selector 或资源 URI
反推 authority。Direct 的保留前缀名称会进行可逆转义，Plugin selector 使用独立编码；规范
Unicode identity 始终保留。Plugin Tool 的有损 slug 和结构边界带稳定 digest，最终投影 collision
在 publication 前拒绝。

## 来源与范围

ADR 0067 是 owner identity 与投影合同；ADR 0012 继续拥有 generation/receipt publication，
ADR 0060 继续拥有 callback cleanup。本修复没有改变 portable `mcp.json` 或
`McpServerConfig.name`，也没有引入 #73 的 Registry/wiring/callback coordinator。

回滚点是本任务分支的 parent revision；交付后应整体 revert 任务 squash commit，不能只移除
typed key 而保留 owner-qualified receipt 或 projection。

## 已知缺口

完整门禁与最终独立复审已通过；PR #141 七项 CI 全绿并以 verified squash commit
`c327f2d0` 进入远端 main。#75 owner-isolation Finding 已闭合，#73 host coordinator 保持独立。
