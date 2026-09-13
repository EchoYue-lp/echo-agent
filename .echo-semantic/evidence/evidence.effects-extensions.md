---
schema_version: 1
id: evidence.effects-extensions
kind: evidence
observed_at: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
source_refs:
  - echo-core/src/tools/mod.rs
  - echo-core/src/tools/permission.rs
  - echo-core/src/tools/artifact.rs
  - echo-core/src/tools/cell.rs
  - echo-execution/src/tools.rs
  - echo-execution/src/sandbox/mod.rs
  - echo-execution/src/sandbox/local.rs
  - echo-orchestration/src/human_loop/service.rs
  - echo-orchestration/src/tasks/command_cell.rs
  - src/agent/react/run/pipeline.rs
  - src/agent/react/subsystems/tool_exec.rs
  - echo-integration/src/mcp/client.rs
  - echo-integration/src/mcp/mod.rs
  - echo-execution/src/skills/hooks.rs
  - echo-execution/src/skills/registry.rs
  - echo-execution/src/skills/external/loader.rs
  - echo-core/src/plugin/registry.rs
  - echo-core/src/plugin/lifecycle.rs
  - src/plugin/prepared.rs
  - echo-integration/src/lsp/manager.rs
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
