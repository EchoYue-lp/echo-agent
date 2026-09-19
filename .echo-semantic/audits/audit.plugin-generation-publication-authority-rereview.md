---
schema_version: 1
id: audit.plugin-generation-publication-authority-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: state_authority
freshness: examined
revision: cb4ee9ed3826fd8055f027e84f69b94dbc329267
finding_refs: [finding.plugin-generation-publication-authority]
challenges:
  agent-target-and-receipt-authority:
    revision: cb4ee9ed3826fd8055f027e84f69b94dbc329267
    source_refs: [src/agent/react/mod.rs, src/plugin/prepared.rs]
    evidence_refs: [evidence.plugin-generation-publication-authority-repair, evidence.plugin-generation-publication-authority-verification]
  independent-generation-and-stale-fencing:
    revision: cb4ee9ed3826fd8055f027e84f69b94dbc329267
    source_refs: [src/plugin/prepared.rs, docs/adr/0012-immutable-plugin-preparation.md]
    evidence_refs: [evidence.plugin-generation-publication-authority-repair, evidence.plugin-generation-publication-authority-verification]
  cancellation-and-cleanup-debt:
    revision: cb4ee9ed3826fd8055f027e84f69b94dbc329267
    source_refs: [src/plugin/prepared.rs, echo-integration/src/mcp/mod.rs]
    evidence_refs: [evidence.plugin-generation-publication-authority-verification, evidence.extension-cleanup-settlement-verification, evidence.foundation-36-72-51-integration-verification]
---

# Plugin generation publication authority 独立复审

## 审查范围

复审 per-ReactAgent publication authority、进程级 generation、receipt token、独立
Integrator、apply/withdraw cancellation、MCP multi-server partial apply、cleanup debt retry，
以及 #73/#75 边界。

## 已检查故障假设

检查独立 Integrator 是否生成不可比较代次；旧 prepared 或 receipt 是否能覆盖或撤销当前代；
跨 Agent、伪造或被修改 receipt 是否可通过；apply/withdraw Future 被取消后是否丢失 cleanup
owner；多 MCP server 的后续连接被取消后是否遗漏已连接 server；close 失败后是否错误放行替代
generation。

## 实际实现路径与证据

ReactAgent 创建唯一 publication authority，所有 Integrator target 都引用该 authority。进程级
checked allocator 为独立 Integrator 分配有序 generation。Apply 在副作用前写入带私有 token
的 canonical cleanup receipt，成功后才推进 latest/active。Rollback 只接受当前 authority 保存的
完整 receipt。取消或 close 失败保留 active/cleanup debt 并阻断替代。MCP server 按名称排序，
新连接在 await 前预留 scope，成功连接在下一次 await 前记入 receipt。

当前 main 上 MCP focused suite 19/19 通过；PR #138 七项 CI 全绿；semantic strict 通过。

## 问题记录

本轮未发现新的 Critical、Important 或 Minor Finding。finding.plugin-generation-publication-authority
具备 repair、verification、independent rereview 与 remote-main delivery 证据，可以标记 resolved。

## 残余风险

裸 MCP server name 的跨 Plugin owner 冲突继续由 finding.plugin-mcp-owner-isolation（#75）追踪。
Registry 持久状态、component wiring 与 callback lifecycle 的统一事务继续由
finding.plugin-lifecycle-coordination（#73）追踪。进程强制退出仍可能截断异步 cleanup。

## 未检查项

未运行真实远端 MCP server、embedding application reload transaction 或跨进程恢复；未复审
#73/#75 的实现候选。
