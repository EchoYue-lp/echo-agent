---
schema_version: 1
id: evidence.mcp-tool-local-classification-repair
kind: evidence
observed_at: 72d1fccf74b85afe9a74e684ca3748b64642affb
source_refs:
  - echo-integration/src/mcp/tool_adapter.rs
  - echo-orchestration/src/human_loop/service.rs
  - src/agent/snapshot.rs
  - src/agent/react/run/pipeline.rs
  - docs/adr/0050-mcp-tool-local-classification.md
supports: [behavior.effect-permission-execution, behavior.extension-publication, rule.permission-effect-order]
limitations:
  - 最终多分支整合仍需统一重生成 Rust public inventory 并执行完整 MR 门禁
  - 未连接第三方生产 MCP server；协议 adapter 与 Agent pipeline 由确定性测试分层组合验证
---

# MCP Tool 本地分类修复证据

## 支持的结论

`McpToolAdapter` 不再把服务端 `readOnlyHint`、`destructiveHint`、`idempotentHint` 或
`openWorldHint` 转换为权限事实。两个默认构造入口均持有本地
`ToolCapabilities(Mutating, Standard, [Write])`；只有 embedding application 显式调用
`with_local_capabilities` 才能基于本地可信策略降级。

同一 capability 快照提供 `ToolAccess`、`ToolRiskLevel`、`ToolPermission`，并决定 transport
error 和 `isError=true` 的 typed side-effect settlement。默认 mutating MCP 失败产生
`PartialSideEffect/Possible`，本地确认的 read-only 工具产生 `Permanent/None`。

`PermissionService` 的同步 mode authority 覆盖 builder、async/sync setter 与 update 写入口。
AgentConfig Plan、显式 plan flag 以及 SDK/host 对 live PermissionService 的 Plan 切换都进入同一
capability-based surface 和执行 hard gate；该 gate 位于 hook 之前，hook Allow 无法覆盖。

## 来源与范围

修复提交为 `ed39fbfb839393eb6a80a89c5be06da1e869900d`、
`ceb3041d86c49562bda0492ea4c6b86d74860be1` 和
`72d1fccf74b85afe9a74e684ca3748b64642affb`。ADR 0050 记录 MCP 官方 annotation trust
约束、framework/application 分层、Plan mode authority 和不门控用户连接的取舍。

## 已知缺口

最终多分支集成会统一重生成 SDK public inventory；本分支不刷新共享 semantic baseline/source
digest。真实第三方 server 互操作、远端 CI 和完整 workspace 门禁留给最终 MR。
