---
schema_version: 1
id: evidence.effects-extensions
kind: evidence
observed_at: f1e9027246760661144786e9e35615cd46d580c6
source_refs:
  - echo-core/src/tools/mod.rs
  - echo-core/src/tools/permission.rs
  - echo-core/src/tools/artifact.rs
  - echo-core/src/tools/cell.rs
  - echo-execution/src/tools.rs
  - echo-execution/src/sandbox/mod.rs
  - echo-execution/src/sandbox/manager.rs
  - echo-execution/src/sandbox/local.rs
  - echo-execution/src/sandbox/k8s.rs
  - echo-orchestration/src/human_loop/service.rs
  - echo-orchestration/src/tasks/command_cell.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/react/builder.rs
  - src/agent/react/subsystems/tool_exec.rs
  - echo-integration/src/mcp/client.rs
  - echo-integration/src/mcp/mod.rs
  - echo-integration/src/mcp/tool_adapter.rs
  - echo-integration/src/mcp/server_config.rs
  - echo-integration/src/mcp/config_loader.rs
  - echo-integration/src/mcp/transport/mod.rs
  - echo-integration/src/mcp/transport/sse.rs
  - echo-integration/src/mcp/transport/stdio.rs
  - echo-integration/src/mcp/transport/http.rs
  - echo-execution/src/skills/hooks.rs
  - echo-execution/src/skills/registry.rs
  - echo-execution/src/skills/external/loader.rs
  - echo-core/src/plugin/registry.rs
  - echo-core/src/plugin/lifecycle.rs
  - src/plugin/prepared.rs
  - echo-integration/src/lsp/client.rs
  - echo-integration/src/lsp/manager.rs
  - echo-integration/src/channels/channels/qq/channel.rs
  - echo-integration/src/channels/channels/feishu/channel.rs
  - echo-sdk-host/src/core_profile/facade/source_operations.rs
  - echo-sdk-host/src/core_profile/facade/integrations.rs
  - echo-sdk-host/src/core_profile/facade/mod.rs
  - echo-tools/src/registry.rs
  - echo-tools/src/shell.rs
  - echo-tools/src/git_worktree.rs
  - echo-agent-learning/tests/example_contracts/demo64_tool_pipeline.rs
  - docs/adr/0002-sandbox-cancellation-cleanup.md
  - docs/adr/0012-immutable-plugin-preparation.md
  - docs/adr/0023-current-skill-frontmatter.md
  - docs/adr/0026-official-skill-frontmatter-only.md
  - docs/adr/0025-deterministic-command-cell-watcher.md
supports: [behavior.effect-permission-execution, behavior.extension-publication, rule.permission-effect-order, rule.extension-generation-authority]
limitations:
  - 应用层的直接用户交互、Workspace policy 和 Device sync 不在 framework 源码中，由适配边界记录
---

# Effect、Permission 与 Extension 证据

## 支持的结论

Tool contract、ToolManager、permission service、sandbox、artifact 和 invocation guard 共同构成外部 effect 路径；MCP、Hook、Skill、Plugin 与 LSP 有各自的发现、注册、发布、撤销和清理 owner。

## 来源与范围

来源覆盖核心 Tool/permission 值、ReactAgent pipeline、programmatic ToolManager、trusted Hook effect、CommandCell、sandbox、MCP manager/client、SkillLoader、PluginRegistry/PreparedPluginSet/LifecycleManager、LSP manager、示例合同及相关 ADR。

## 已知缺口

完整 permission precedence 与 Plugin 跨重启 cleanup debt 仍需定向审计；本 Evidence 不引入面向公网或多租户的产品策略。
