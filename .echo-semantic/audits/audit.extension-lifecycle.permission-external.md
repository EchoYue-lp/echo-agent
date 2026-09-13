---
schema_version: 1
id: audit.extension-lifecycle.permission-external
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: permission_external
freshness: examined
revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
finding_refs: [finding.hook-permission-precedence, finding.hook-protected-path, finding.mcp-client-capability-advertisement, finding.mcp-tool-permission-classification, finding.extension-credential-debug-redaction]
challenges:
  hook-trusted-effect-versus-agent-authorization:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-execution/src/skills/hooks.rs, src/agent/react/run/pipeline.rs, echo-orchestration/src/human_loop/service.rs]
    evidence_refs: [evidence.effects-extensions]
  mcp-tool-classification:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-integration/src/mcp/tool_adapter.rs, echo-core/src/tools/mod.rs, src/agent/snapshot.rs, src/agent/react/run/pipeline.rs]
    evidence_refs: [evidence.effects-extensions]
  credential-debug-redaction:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-integration/src/mcp/server_config.rs, echo-integration/src/mcp/config_loader.rs, echo-integration/src/mcp/transport/http.rs, echo-integration/src/channels/channels/qq/channel.rs, echo-integration/src/channels/channels/feishu/channel.rs]
    evidence_refs: [evidence.effects-extensions]
---

# Extension 权限、MCP Tool 与 Credential 审计

## 审查范围

审查 trusted Hook effect 与 Agent authorization 的边界、MCP server annotation 到 ToolPermission 的映射，以及 MCP/QQ/Feishu credential Debug/logging。

## 已检查故障假设

验证 trusted extension 是否被误加产品门控、Hook Permission 是否绕过 Agent protected path/deny、MCP annotation 是否被当作权限事实，以及公开 config Debug 是否暴露 secret。

## 实际实现路径与证据

用户主动配置 Hook/MCP 是本地 trusted extension，不应增加连接门控；但 Hook Permission result 授权后续 Agent Tool 时必须服从唯一 policy。McpToolAdapter 信任 server annotation 设置风险但 permissions 为空，Plan mode 也漏 mcp 工具，伪造 readOnlyHint 会改变副作用分类。多个 MCP/QQ/Feishu config 派生原始 Debug，HTTP transport 还记录 session ID；仅部分 QQ response 有局部脱敏。

## 问题记录

确认 Hook 与 MCP capability Findings；新增 MCP Tool permission classification 与 credential Debug redaction。密钥不进日志是本地仍成立的保护，不依赖公网威胁模型。

## 残余风险

MCP server metadata 只能作为提示，不能替代 embedding application/Agent policy；credential risk 当前是确定暴露面，不等于已证明生产事故。

## 未检查项

未检查 EKO 如何打印/上报这些 config，未做真实 MCP 互操作或第三方 tracing subscriber 审计。
