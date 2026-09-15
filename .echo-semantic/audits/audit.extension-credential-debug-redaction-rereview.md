---
schema_version: 1
id: audit.extension-credential-debug-redaction-rereview
kind: audit
boundary_ref: boundary.extension-lifecycle
lens: permission_external
freshness: examined
revision: 98a2e11cfb6e88e2f310ae2c2b40cd9e009534a4
finding_refs: [finding.extension-credential-debug-redaction]
challenges:
  config-debug-and-exact-secrets:
    revision: 98a2e11cfb6e88e2f310ae2c2b40cd9e009534a4
    source_refs: [echo-integration/src/redaction.rs, echo-integration/src/mcp/server_config.rs, echo-integration/src/mcp/config_loader.rs, echo-integration/src/channels/channels/qq/channel.rs, echo-integration/src/channels/channels/feishu/channel.rs]
    evidence_refs: [evidence.extension-credential-debug-redaction-repair, evidence.extension-credential-debug-redaction-verification]
  transport-error-and-url-redaction:
    revision: 98a2e11cfb6e88e2f310ae2c2b40cd9e009534a4
    source_refs: [echo-integration/src/mcp/transport/mod.rs, echo-integration/src/mcp/transport/http.rs, echo-integration/src/mcp/transport/sse.rs, echo-integration/src/mcp/transport/stdio.rs, echo-integration/src/channels/channels/qq/api.rs, echo-integration/src/channels/channels/qq/gateway.rs, echo-integration/src/channels/channels/feishu/api.rs, echo-integration/src/channels/channels/feishu/long_poll.rs, echo-integration/src/channels/channels/feishu/webhook.rs]
    evidence_refs: [evidence.extension-credential-debug-redaction-repair, evidence.extension-credential-debug-redaction-verification]
  global-retention-and-permission-boundary:
    revision: 98a2e11cfb6e88e2f310ae2c2b40cd9e009534a4
    source_refs: [echo-core/src/utils/retention.rs, src/trace/mod.rs, echo-state/src/audit/file.rs, echo-integration/src/lib.rs]
    evidence_refs: [evidence.extension-credential-debug-redaction-repair, evidence.extension-credential-debug-redaction-verification]
---

# Extension credential diagnostic redaction 独立复审

## 审查范围

独立 reviewer 审查 Finding #56 的最终 diff、MCP/QQ/Feishu 真实调用路径、配置 Debug、原始日志和返回错误、UTF-8/retention 边界、feature 组合及文档。

## 已检查故障假设

检查普通 session id 是否被误当全局 secret 而破坏持久化查询、reqwest URL 是否绕过 shape redaction、opaque secret 是否因字段名缺失或 Authorization scheme 被漏掉、重叠 secret 与截断顺序是否留下前后缀、无效 URL 是否 fail-open、no-default build 是否产生 dead-code warning，以及 redaction 是否改变真实请求或增加权限门控。

## 实际实现路径与证据

最终实现仅在 integration 适配边界隐藏 credential。exact replacement 在 bounded retention 前执行，使用 UTF-8 字符序列最长单次匹配；URL parse failure 固定隐藏，reqwest URL 单独清洗；transport 只修改 error diagnostics，不修改成功 payload 或 wire request。普通 session id 未进入共享 retention，模块只在 MCP/channel feature 下编译。

## 问题记录

复审期间发现并修复全局 session correlation 回归、reqwest attached URL、no-default dead-code、截断顺序、invalid URL、QQ gateway exact token、MCP server metadata、JSON error data、Authorization payload和重叠 secret 等反例。最终结论为 pass，无阻断项或后续建议。

## 残余风险

用户显式序列化原始 config、第三方 subscriber 的自定义字段和未经过框架 transport 的 embedding application 日志不属于本合同。

## 未检查项

未做真实第三方服务互操作、完整 workspace 合并门禁、远端 CI 或 `finding.trace-audit-secret-boundary` 的 backend 全量审计。
