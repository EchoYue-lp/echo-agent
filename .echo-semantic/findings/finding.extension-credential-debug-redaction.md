---
schema_version: 1
id: finding.extension-credential-debug-redaction
kind: finding
type: intent_gap
status: resolved
severity: high
primary_focus: permission_external
focus: [data_durability, contract_evidence, result_side_effect]
boundary_ref: boundary.extension-lifecycle
behavior_refs: [behavior.extension-publication]
rule_refs: [rule.extension-generation-authority]
evidence_refs: [evidence.effects-extensions, evidence.extension-credential-debug-redaction-repair, evidence.extension-credential-debug-redaction-verification]
audit_refs: [audit.extension-lifecycle.permission-external, audit.extension-credential-debug-redaction-rereview]
decision_refs: []
repair_evidence_refs: [evidence.extension-credential-debug-redaction-repair]
verification_evidence_refs: [evidence.extension-credential-debug-redaction-verification]
rereview_audit_refs: [audit.extension-credential-debug-redaction-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Extension credential 配置缺统一 Debug/redaction 合同

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/56

## 问题

MCP env/Authorization、QQ client_secret、Feishu app_secret/verification_token/signing_key 等公开 config 派生原始 Debug；HTTP transport 还记录 session ID，仅部分 backend 手工脱敏。

## 触发条件与影响

错误上报、diagnostic 或 embedding application 打印配置时可泄漏本地 secret；这是确定暴露面，但当前未证明完整配置已在生产日志发生。

## 证据

MCP server/config/HTTP transport 与 QQ/Feishu channel config 源码展示字段和 Debug/logging 行为。

## 处理记录

Permission Audit 确认；密钥不进日志是本地仍成立的保护，后续建立 Secret wrapper/redacted Debug 与测试，不增加扩展连接门控。

当前由 `echo-integration` 的 feature-gated redaction helper 统一处理配置 Debug、exact credential、URL、reqwest error 和 MCP JSON-RPC error；QQ/Feishu/MCP focused tests、all-feature crate tests、Clippy、no-default check 与独立复审闭合本 Finding。未增加连接或权限门控，也未修改共享 Trace/Audit retention。
